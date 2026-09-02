# Standard Font Selection Strategies (cross-engine diff)

当 PDF 不嵌入字体时，渲染器必须选择替代字体。三个参考引擎的选择策略如下，
xpdf 通过 `Device::resolve_standard_font_data` hook + adapter 侧 resolver 函数支持不同策略，
使像素级对比有据可查。

## 共同点

- **嵌入字体优先**：三者的第一选择都是从 FontDescriptor/FontFile 提取嵌入数据。
- **Symbol / ZapfDingbats**：三者都使用 Foxit PFB（无 Liberation 等价物）。
- **Base-14 别名表**：三者都有一个大的 BaseFont 名字→标准 14 字体的映射表（~70-90 条），
  来源均为 Apache PDFBox 的 `FontMapperImpl`。

## 差异

### 1. hayro

- **字体数据**：14 个 Foxit PFB（`include_bytes!` 编译进 `hayro-interpret`，`embed-fonts` feature）。
- **无系统字体路径**：`FontResolverFn` 回调是唯一的外部数据来源；默认 resolver
  只响应 `Standard` 和 `Fallback` query，返回编译进二进制的 Foxit PFB。
- **名称匹配**：`select_standard_font()` 精确匹配 BaseFont 名（~70 条 alias，引用 PDFBox）。
- **Fallback 匹配**（名称不在 alias 表中）：`FallbackFontQuery::new(dict)` 从
  FontDescriptor 提取 flags（FixedPitch bit 1、Serif bit 6）和 name substring（Bold/Italic），
  然后 `pick_standard_font()` 按 is_fixed_pitch → !is_serif → serif 三级选择 14 种标准字体。
- **宽度补偿**：fallback 路径会对替代字形做水平 stretch（`declared_width / standard_font_width`），
  使文本布局与 PDF 声明的宽度一致。xpdf 目前不做。

  源码：`hayro/hayro-interpret/src/font/standard_font.rs`（alias 表 + Foxit 数据）、
  `hayro/hayro-interpret/src/font/mod.rs:414-538`（FallbackFontQuery）、
  `hayro/hayro-interpret/src/font/type1.rs:23-72`（加载流程：名称匹配 → 嵌入 → fallback）。

### 2. pdf.js

- **字体数据**：14 个 Foxit PFB + 4 个 Liberation Sans TTF（`standard_fonts/` 目录）。
- **系统字体路径**：`font_substitutions.js` 生成 CSS `@font-face` 规则，`local()` 优先查找系统字体，
  `url()` 加载 bundled 数据。这是 Web 端唯一可用的路径（浏览器 Canvas API 需要通过
  CSS font-face 注入字体）。
- **名称匹配**：`font_substitutions.js` 的 `substitutionMap`（~50+ 条），结构与 hayro 类似但
  也包含 CJK 字体映射（HeiseiMin/STSong 等）和系统字体候选列表。
- **Liberation 使用**：**仅 Helvetica 家族**有 Liberation TTF path（
  `Helvetica→LiberationSans-Regular.ttf`，Bold/Italic/BoldItalic 同理），且有
  `helvetica_factors.js` 做 glyph 级 metric rescaling。Times 和 Courier **无** Liberation path，
  只有 `local()` 系统字体候选列表。
- **宽度补偿**：通过 `helvetica_factors.js` / `segoeui_factors.js` / `myriadpro_factors.js`
  中的 per-glyph 缩放因子实现（与 hayro 的统一 stretch 比率不同）。

  源码：`pdf.js/src/core/font_substitutions.js`（名称映射 + Liberation 路径）、
  `pdf.js/src/core/helvetica_factors.js`（metric rescaling）、
  `pdf.js/src/core/font.js`（字体加载主逻辑）。

### 3. pdfium

- **字体数据**：14 个 Foxit CFF（编译为 C byte arrays 在 `core/fxge/fontdata/chromefontdata/`）
  + 2 个 Foxit Multiple Master CFF（Chrome Sans MM / Chrome Serif MM，~67KB / ~113KB，
  用于非 Base-14 的通用 fallback，可通过 MM axis 合成任意 weight/width）。
- **系统字体 + bundled 双路径**：初始化时安装平台 `SystemFontInfoIface`（Linux 扫描
  `/usr/share/fonts` 等；macOS/Windows 用平台 API）。系统查找失败时回退到 bundled Foxit。
- **名称匹配**：`GetSubstName()` → `GetStandardFontName()` 二分搜索 89 条 alias 表 →
  Base-14 规范名。然后 **per-platform clone 名替换**（Helvetica→Arial, Courier→Courier New,
  Times→Times New Roman）用于系统字体查找。
- **Fallback 匹配**：`CFX_FolderFontInfo::FindFont()` 的 `GetSimilarValue()` 加权评分：
  精确名字匹配 +4，bold 匹配 +16，italic 匹配 +16，serif 匹配 +16，script +8，
  fixed-pitch +8。**分类信号来自 FontDescriptor flags**（FixedPitch/Serif/Script bits）
  和 FontWeight，而非名字 substring。
- **合成 embolden/oblique**：如果系统字体 weight/italic 不匹配请求，
  `UseExternalSubst()` 记录 `m_Weight` 和 `m_ItalicAngle`（默认 -12°），渲染时
  作为 transform 应用。

  源码（`/home/gaopeng/Enter/pdfium-lib/build/wasm32/pdfium/`）：
  `core/fxge/cfx_fontmapper.cpp:567-748`（FindSubstFont 主算法）、
  `core/fxge/cfx_folderfontinfo.cpp:29-145`（系统字体匹配 + 评分）、
  `core/fxge/fontdata/chromefontdata/`（bundled Foxit CFF + MM 数据）。

## xpdf 当前的标准字体路径

```
load_font_from_resources → 嵌入数据? → device.load_font_data() → done
  → 否则 → device.resolve_standard_font_data()  ← adapter hook（优先）
    → None? → bundled_standard_font_data()          ← 14 Foxit PFB（编译进 core）
               → known-alias 表 (~68 条) 匹配
               → else pick_fallback_standard_font_key()  ← name substring heuristic
```

默认行为（无 resolver）= Foxit PFB，与 hayro `embed-fonts` 一致。
Adapter 侧已有 `SkiaDevice::set_standard_font_resolver()` 可安装自定义 resolver。
`pdf-x-device-tinyskia/src/skia_device/standard_font_resolvers.rs` 提供
`liberation_ttf_resolver()` 读取磁盘 Liberation TTF（12 文件）。

## 策略对比速查

| | 字体数据 | 名称匹配 | Fallback 信号 | 宽度补偿 |
|---|----------|---------|-------------|--------|
| **hayro** | Foxit PFB (built-in) | alias 表 (~70) | FontDescriptor flags + name | glyph stretch |
| **pdf.js** | Foxit PFB + Liberation Sans TTF | alias 表 (~50+) + CSS local() | name substring (bold/italic regex) | per-glyph factors |
| **pdfium** | Foxit CFF + MM CFF (built-in) + system | alias 表 (89) + platform clone | flags + weighted scoring | synthetic weight/italic |
| **xpdf (default)** | Foxit PFB (built-in) | alias 表 (~68) | name substring | 无 |
