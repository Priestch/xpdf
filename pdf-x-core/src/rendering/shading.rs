//! PDF shading rasterization for Type 2 (axial) and Type 3 (radial) shadings.
//!
//! References: PDF 1.7 spec §8.7.4.5 (Type 2) and §8.7.4.6 (Type 3).
//! Algorithm adapted from hayro `shading.rs` and pdf.js `pattern.js`.

/// Pre-parsed shading data ready for pixel sampling.
#[derive(Debug, Clone)]
pub struct Shading {
    pub kind: ShadingKind,
    /// Number of components in the shading color space.
    pub n_components: usize,
    pub color_space: ShadingColorSpace,
    /// Optional clip rectangle in shading space [x0, y0, x1, y1].
    pub bbox: Option<[f64; 4]>,
    /// Pattern matrix (maps shading space → user space). Identity = [1,0,0,1,0,0].
    pub matrix: [f64; 6],
    /// Optional background color for samples outside the active shading domain.
    pub background: Option<Vec<f64>>,
    /// Function-based shading patterns are anchored to the page/root transform, while
    /// axial/radial patterns follow the current content CTM path used by existing tests.
    pub use_root_ctm: bool,
}

#[derive(Debug, Clone)]
pub enum ShadingColorSpace {
    DeviceGray,
    DeviceRGB,
    DeviceCMYK,
    CalRGB {
        white_point: [f64; 3],
        black_point: [f64; 3],
        gamma: [f64; 3],
        matrix: [f64; 9],
    },
    Alternate {
        n_components: usize,
        base: Box<ShadingColorSpace>,
        tint_function: ShadingFunction,
    },
}

impl ShadingColorSpace {
    pub fn n_components(&self) -> usize {
        match self {
            ShadingColorSpace::DeviceGray => 1,
            ShadingColorSpace::DeviceRGB => 3,
            ShadingColorSpace::DeviceCMYK => 4,
            ShadingColorSpace::CalRGB { .. } => 3,
            ShadingColorSpace::Alternate { n_components, .. } => *n_components,
        }
    }

    pub fn to_rgba(&self, components: &[f64]) -> (u8, u8, u8, u8) {
        let clamp = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        match self {
            ShadingColorSpace::DeviceGray => {
                let g = clamp(components.first().copied().unwrap_or(0.0));
                (g, g, g, 255)
            }
            ShadingColorSpace::DeviceRGB => (
                clamp(components.first().copied().unwrap_or(0.0)),
                clamp(components.get(1).copied().unwrap_or(0.0)),
                clamp(components.get(2).copied().unwrap_or(0.0)),
                255,
            ),
            ShadingColorSpace::DeviceCMYK => {
                let c = components.first().copied().unwrap_or(0.0);
                let m = components.get(1).copied().unwrap_or(0.0);
                let y = components.get(2).copied().unwrap_or(0.0);
                let k = components.get(3).copied().unwrap_or(0.0);
                #[cfg(feature = "rendering")]
                {
                    let (r, g, b) = crate::rendering::cmyk_icc::cmyk_pixel_to_rgb(
                        c as f32, m as f32, y as f32, k as f32,
                    );
                    (r, g, b, 255)
                }
                #[cfg(not(feature = "rendering"))]
                {
                    let r = (1.0 - c) * (1.0 - k);
                    let g = (1.0 - m) * (1.0 - k);
                    let b = (1.0 - y) * (1.0 - k);
                    (clamp(r), clamp(g), clamp(b), 255)
                }
            }
            ShadingColorSpace::CalRGB {
                white_point,
                black_point,
                gamma,
                matrix,
            } => {
                let [r, g, b] = calrgb_to_rgb(components, white_point, black_point, gamma, matrix);
                (clamp(r), clamp(g), clamp(b), 255)
            }
            ShadingColorSpace::Alternate {
                base,
                tint_function,
                ..
            } => {
                let tinted = tint_function.eval_inputs(components);
                base.to_rgba(&tinted)
            }
        }
    }
}

#[derive(Debug, Clone)]
pub enum ShadingKind {
    /// Type 1: function-based shading over a rectangular domain.
    FunctionBased {
        domain: [f64; 4],
        function: ShadingFunction,
    },
    /// Type 2: linear gradient along a line from (x0,y0) to (x1,y1).
    Axial {
        x0: f64,
        y0: f64,
        x1: f64,
        y1: f64,
        t0: f64,
        t1: f64,
        function: ShadingFunction,
        extend_start: bool,
        extend_end: bool,
    },
    /// Type 3: radial gradient between two circles.
    Radial {
        x0: f64,
        y0: f64,
        r0: f64,
        x1: f64,
        y1: f64,
        r1: f64,
        t0: f64,
        t1: f64,
        function: ShadingFunction,
        extend_start: bool,
        extend_end: bool,
    },
    /// Type 4: free-form Gouraud-shaded triangle mesh.
    TriangleMesh {
        triangles: Vec<ShadingTriangle>,
        function: Option<ShadingFunction>,
    },
}

#[derive(Debug, Clone)]
pub struct ShadingTriangle {
    pub p0: ShadingVertex,
    pub p1: ShadingVertex,
    pub p2: ShadingVertex,
}

#[derive(Debug, Clone)]
pub struct ShadingVertex {
    pub x: f64,
    pub y: f64,
    pub components: Vec<f64>,
}

impl ShadingTriangle {
    pub fn sample(&self, x: f64, y: f64) -> Option<Vec<f64>> {
        let ax = self.p0.x;
        let ay = self.p0.y;
        let bx = self.p1.x;
        let by = self.p1.y;
        let cx = self.p2.x;
        let cy = self.p2.y;
        let v0x = bx - ax;
        let v0y = by - ay;
        let v1x = cx - ax;
        let v1y = cy - ay;
        let v2x = x - ax;
        let v2y = y - ay;
        let d00 = v0x * v0x + v0y * v0y;
        let d01 = v0x * v1x + v0y * v1y;
        let d11 = v1x * v1x + v1y * v1y;
        let d20 = v2x * v0x + v2y * v0y;
        let d21 = v2x * v1x + v2y * v1y;
        let denom = d00 * d11 - d01 * d01;
        if denom.abs() < 1e-12 {
            return None;
        }

        let v = (d11 * d20 - d01 * d21) / denom;
        let w = (d00 * d21 - d01 * d20) / denom;
        let u = 1.0 - v - w;
        const EPS: f64 = -1e-6;
        if u < EPS || v < EPS || w < EPS {
            return None;
        }

        let n = self
            .p0
            .components
            .len()
            .min(self.p1.components.len())
            .min(self.p2.components.len());
        if n == 0 {
            return None;
        }

        Some(
            (0..n)
                .map(|i| {
                    u * self.p0.components[i]
                        + v * self.p1.components[i]
                        + w * self.p2.components[i]
                })
                .collect(),
        )
    }
}

/// A 1-in -> N-out function used by shadings.
#[derive(Debug, Clone)]
pub enum ShadingFunction {
    /// Exponential interpolation: y_j = C0_j + t^N * (C1_j - C0_j)
    Exponential { c0: Vec<f64>, c1: Vec<f64>, n: f64 },
    /// PostScript calculator function (Type 4).
    PostScript {
        program: Vec<u8>,
        domain: Vec<f64>,
        range: Vec<f64>,
    },
    /// Stitching function (Type 3) — combines multiple sub-functions.
    Stitching {
        functions: Vec<ShadingFunction>,
        bounds: Vec<f64>,
        encode: Vec<f64>,
        domain: [f64; 2],
    },
    /// Sampled function (Type 0).
    Sampled {
        domain: Vec<f64>,
        range: Vec<f64>,
        size: Vec<usize>,
        bits_per_sample: usize,
        encode: Vec<f64>,
        decode: Vec<f64>,
        samples: Vec<f64>,
    },
}

impl ShadingFunction {
    /// Evaluate the function at parameter `t`, returning color components.
    pub fn eval(&self, t: f64) -> Vec<f64> {
        self.eval_inputs(&[t])
    }

    /// Evaluate the function with arbitrary input arity.
    pub fn eval_inputs(&self, inputs: &[f64]) -> Vec<f64> {
        match self {
            ShadingFunction::Exponential { c0, c1, n } => {
                let t = inputs.first().copied().unwrap_or(0.0).clamp(0.0, 1.0);
                let s = if (n - 1.0).abs() < 1e-10 {
                    t
                } else {
                    t.powf(*n)
                };
                c0.iter()
                    .zip(c1.iter())
                    .map(|(a, b)| a + s * (b - a))
                    .collect()
            }
            ShadingFunction::Stitching {
                functions,
                bounds,
                encode,
                domain,
            } => {
                let t = inputs.first().copied().unwrap_or(0.0);
                let t = t.clamp(domain[0], domain[1]);
                let mut idx = 0;
                for (i, &bound) in bounds.iter().enumerate() {
                    if t >= bound {
                        idx = i + 1;
                    }
                }
                if idx >= functions.len() {
                    idx = functions.len() - 1;
                }
                let d0 = if idx == 0 { domain[0] } else { bounds[idx - 1] };
                let d1 = if idx == bounds.len() {
                    domain[1]
                } else {
                    bounds[idx]
                };
                let t2 = if (d1 - d0).abs() < 1e-12 {
                    0.0
                } else {
                    (t - d0) / (d1 - d0)
                };
                let e0 = encode.get(idx * 2).copied().unwrap_or(0.0);
                let e1 = encode.get(idx * 2 + 1).copied().unwrap_or(1.0);
                let sub_t = e0 + t2 * (e1 - e0);
                functions[idx].eval(sub_t)
            }
            ShadingFunction::PostScript {
                program,
                domain,
                range,
            } => {
                let program = std::str::from_utf8(program).unwrap_or("");
                let num_out = range.len() / 2;
                let clamped_inputs = clamp_function_values(inputs, domain);
                let mut output = eval_postscript(program, &clamped_inputs, num_out);
                clamp_function_values_in_place(&mut output, range);
                output
            }
            ShadingFunction::Sampled {
                domain,
                range,
                size,
                encode,
                decode,
                samples,
                ..
            } => eval_sampled_function(inputs, domain, range, size, encode, decode, samples),
        }
    }
}

fn eval_sampled_function(
    inputs: &[f64],
    domain: &[f64],
    range: &[f64],
    size: &[usize],
    encode: &[f64],
    decode: &[f64],
    samples: &[f64],
) -> Vec<f64> {
    let input_size = domain.len() / 2;
    let output_size = range.len() / 2;
    if input_size == 0
        || output_size == 0
        || size.len() < input_size
        || encode.len() < input_size * 2
        || decode.len() < output_size * 2
        || samples.is_empty()
        || input_size >= usize::BITS as usize
    {
        return vec![0.0; output_size.max(1)];
    }

    // Matches pdf.js `constructSampled`: build all hypercube vertices, weight
    // them by linear interpolation factors, then Decode and Range-clamp.
    let cube_vertices = 1usize << input_size;
    let mut cube_weights = vec![1.0; cube_vertices];
    let mut cube_offsets = vec![0usize; cube_vertices];
    let mut stride = output_size;
    let mut bit = 1usize;

    for i in 0..input_size {
        let d0 = domain[i * 2];
        let d1 = domain[i * 2 + 1];
        let input = inputs
            .get(i)
            .copied()
            .unwrap_or(d0)
            .clamp(d0.min(d1), d0.max(d1));
        let encoded = interpolate(input, d0, d1, encode[i * 2], encode[i * 2 + 1]);
        let size_i = size[i].max(1);
        let encoded = encoded.clamp(0.0, size_i as f64 - 1.0);

        let e0 = if encoded < size_i as f64 - 1.0 {
            encoded.floor()
        } else {
            (encoded - 1.0).max(0.0)
        };
        let n0 = e0 + 1.0 - encoded;
        let n1 = encoded - e0;
        let offset0 = e0 as usize * stride;
        let offset1 = offset0 + stride;

        for vertex in 0..cube_vertices {
            if vertex & bit != 0 {
                cube_weights[vertex] *= n1;
                cube_offsets[vertex] = cube_offsets[vertex].saturating_add(offset1);
            } else {
                cube_weights[vertex] *= n0;
                cube_offsets[vertex] = cube_offsets[vertex].saturating_add(offset0);
            }
        }

        stride = stride.saturating_mul(size_i);
        bit <<= 1;
    }

    (0..output_size)
        .map(|component| {
            let mut value = 0.0;
            for vertex in 0..cube_vertices {
                value += samples
                    .get(cube_offsets[vertex] + component)
                    .copied()
                    .unwrap_or(0.0)
                    * cube_weights[vertex];
            }

            let decoded = interpolate(
                value,
                0.0,
                1.0,
                decode[component * 2],
                decode[component * 2 + 1],
            );
            decoded.clamp(
                range[component * 2].min(range[component * 2 + 1]),
                range[component * 2].max(range[component * 2 + 1]),
            )
        })
        .collect()
}

fn interpolate(x: f64, xmin: f64, xmax: f64, ymin: f64, ymax: f64) -> f64 {
    if (xmax - xmin).abs() < f64::EPSILON {
        ymin
    } else {
        ymin + (x - xmin) * ((ymax - ymin) / (xmax - xmin))
    }
}

fn clamp_function_values(values: &[f64], bounds: &[f64]) -> Vec<f64> {
    values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let low = bounds.get(index * 2).copied();
            let high = bounds.get(index * 2 + 1).copied();
            match (low, high) {
                (Some(low), Some(high)) => value.clamp(low.min(high), low.max(high)),
                _ => *value,
            }
        })
        .collect()
}

fn clamp_function_values_in_place(values: &mut [f64], bounds: &[f64]) {
    for (index, value) in values.iter_mut().enumerate() {
        let low = bounds.get(index * 2).copied();
        let high = bounds.get(index * 2 + 1).copied();
        if let (Some(low), Some(high)) = (low, high) {
            *value = value.clamp(low.min(high), low.max(high));
        }
    }
}

#[derive(Debug, Clone)]
enum PsOp {
    Number(f64),
    Procedure(Vec<PsOp>),
    Name(String),
}

#[derive(Debug, Clone)]
enum PsValue {
    Number(f64),
    Bool(bool),
    Procedure(Vec<PsOp>),
}

impl PsValue {
    fn as_number(&self) -> f64 {
        match self {
            PsValue::Number(v) => *v,
            PsValue::Bool(v) => {
                if *v {
                    1.0
                } else {
                    0.0
                }
            }
            PsValue::Procedure(_) => 0.0,
        }
    }

    fn as_bool(&self) -> bool {
        match self {
            PsValue::Number(v) => *v != 0.0,
            PsValue::Bool(v) => *v,
            PsValue::Procedure(_) => false,
        }
    }
}

/// Evaluate a PostScript calculator function (PDF Function Type 4).
///
/// `num_out` is the expected number of output values (number of Range
/// pairs). Per the PDF spec and pdf.js, only the top `num_out` numeric
/// values on the stack are returned — conditional programs that leave
/// residue underneath are handled correctly.
/// Source: pdf.js `pdf.js/src/core/function.js` `postScriptEval`.
fn eval_postscript(program: &str, inputs: &[f64], num_out: usize) -> Vec<f64> {
    let mut tokens = PsTokenizer::new(program);
    let ops = if program.trim_start().starts_with('{') {
        match tokens.next() {
            Some("{") => parse_ps_procedure(&mut tokens, true).unwrap_or_default(),
            _ => Vec::new(),
        }
    } else {
        parse_ps_procedure(&mut tokens, false).unwrap_or_default()
    };
    let mut stack: Vec<PsValue> = inputs.iter().copied().map(PsValue::Number).collect();

    eval_ps_ops(&ops, &mut stack, 0);

    // Extract only the top `num_out` numeric values from the stack.
    // pdf.js pops exactly `numOut` from the top; collecting in reverse
    // and re-reversing is equivalent.
    let mut numbers: Vec<f64> = stack
        .iter()
        .rev()
        .filter_map(|v| match v {
            PsValue::Number(n) => Some(*n),
            PsValue::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
            PsValue::Procedure(_) => None,
        })
        .take(num_out)
        .collect();
    numbers.reverse();
    numbers
}

struct PsTokenizer<'a> {
    input: &'a str,
    index: usize,
}

impl<'a> PsTokenizer<'a> {
    fn new(input: &'a str) -> Self {
        Self { input, index: 0 }
    }

    fn next(&mut self) -> Option<&'a str> {
        self.skip_ws_and_comments();
        if self.index >= self.input.len() {
            return None;
        }

        let start = self.index;
        let first = self.input[start..].chars().next()?;
        if first == '{' || first == '}' {
            self.index += first.len_utf8();
            return Some(&self.input[start..self.index]);
        }

        while self.index < self.input.len() {
            let ch = self.input[self.index..].chars().next()?;
            if ch.is_whitespace() || ch == '{' || ch == '}' || ch == '%' {
                break;
            }
            self.index += ch.len_utf8();
        }

        Some(&self.input[start..self.index])
    }

    fn skip_ws_and_comments(&mut self) {
        while self.index < self.input.len() {
            let ch = match self.input[self.index..].chars().next() {
                Some(ch) => ch,
                None => break,
            };

            if ch.is_whitespace() {
                self.index += ch.len_utf8();
                continue;
            }

            if ch == '%' {
                while self.index < self.input.len() {
                    let ch = match self.input[self.index..].chars().next() {
                        Some(ch) => ch,
                        None => break,
                    };
                    self.index += ch.len_utf8();
                    if ch == '\n' || ch == '\r' {
                        break;
                    }
                }
                continue;
            }

            break;
        }
    }
}

fn parse_ps_procedure(tokens: &mut PsTokenizer<'_>, stop_on_close: bool) -> Option<Vec<PsOp>> {
    let mut ops = Vec::new();

    while let Some(token) = tokens.next() {
        match token {
            "{" => ops.push(PsOp::Procedure(parse_ps_procedure(tokens, true)?)),
            "}" if stop_on_close => return Some(ops),
            "}" => return Some(ops),
            _ => {
                if let Ok(number) = token.parse::<f64>() {
                    ops.push(PsOp::Number(number));
                } else {
                    ops.push(PsOp::Name(token.to_string()));
                }
            }
        }
    }

    if stop_on_close { None } else { Some(ops) }
}

fn eval_ps_ops(ops: &[PsOp], stack: &mut Vec<PsValue>, depth: usize) {
    if depth > 32 {
        return;
    }

    for op in ops {
        match op {
            PsOp::Number(v) => stack.push(PsValue::Number(*v)),
            PsOp::Procedure(proc) => stack.push(PsValue::Procedure(proc.clone())),
            PsOp::Name(name) => eval_ps_name(name, stack, depth),
        }
    }
}

fn eval_ps_name(name: &str, stack: &mut Vec<PsValue>, depth: usize) {
    match name {
        "true" => stack.push(PsValue::Bool(true)),
        "false" => stack.push(PsValue::Bool(false)),
        "dup" => {
            if let Some(v) = stack.last().cloned() {
                stack.push(v);
            }
        }
        "exch" => {
            let len = stack.len();
            if len >= 2 {
                stack.swap(len - 1, len - 2);
            }
        }
        "pop" => {
            stack.pop();
        }
        "copy" => {
            if let Some(n) = pop_usize(stack) {
                let len = stack.len();
                if n <= len {
                    let copied = stack[len - n..].to_vec();
                    stack.extend(copied);
                }
            }
        }
        "index" => {
            if let Some(n) = pop_usize(stack) {
                if n < stack.len() {
                    stack.push(stack[stack.len() - 1 - n].clone());
                }
            }
        }
        "roll" => {
            if stack.len() >= 2 {
                let j = stack.pop().map(|v| v.as_number()).unwrap_or(0.0) as i32;
                let n = stack.pop().map(|v| v.as_number()).unwrap_or(0.0) as usize;
                if n > 0 && n <= stack.len() {
                    let len = stack.len();
                    if j >= 0 {
                        stack[len - n..].rotate_right(j as usize % n);
                    } else {
                        stack[len - n..].rotate_left((-j) as usize % n);
                    }
                }
            }
        }
        "if" => {
            let proc = match stack.pop() {
                Some(PsValue::Procedure(proc)) => proc,
                _ => return,
            };
            if stack.pop().map(|v| v.as_bool()).unwrap_or(false) {
                eval_ps_ops(&proc, stack, depth + 1);
            }
        }
        "ifelse" => {
            let false_proc = match stack.pop() {
                Some(PsValue::Procedure(proc)) => proc,
                _ => return,
            };
            let true_proc = match stack.pop() {
                Some(PsValue::Procedure(proc)) => proc,
                _ => return,
            };
            let cond = stack.pop().map(|v| v.as_bool()).unwrap_or(false);
            eval_ps_ops(
                if cond { &true_proc } else { &false_proc },
                stack,
                depth + 1,
            );
        }
        "add" => ps_bin_number(stack, |a, b| a + b),
        "sub" => ps_bin_number(stack, |a, b| a - b),
        "mul" => ps_bin_number(stack, |a, b| a * b),
        "div" => ps_bin_number(stack, |a, b| a / b),
        "idiv" => ps_bin_number(stack, |a, b| {
            if b == 0.0 {
                0.0
            } else {
                (a as i64 / b as i64) as f64
            }
        }),
        "mod" => ps_bin_number(stack, |a, b| a % b),
        "neg" => ps_unary_number(stack, |a| -a),
        "abs" => ps_unary_number(stack, |a| a.abs()),
        "sqrt" => ps_unary_number(stack, |a| a.sqrt()),
        "exp" => ps_bin_number(stack, |a, b| a.powf(b)),
        "ln" => ps_unary_number(stack, |a| a.ln()),
        "log" => ps_unary_number(stack, |a| a.log10()),
        "sin" => ps_unary_number(stack, |a| a.to_radians().sin()),
        "cos" => ps_unary_number(stack, |a| a.to_radians().cos()),
        "atan" => ps_bin_number(stack, |a, b| {
            let mut deg = a.atan2(b).to_degrees() % 360.0;
            if deg < 0.0 {
                deg += 360.0;
            }
            deg
        }),
        "floor" => ps_unary_number(stack, |a| a.floor()),
        "ceiling" => ps_unary_number(stack, |a| a.ceil()),
        "round" => ps_unary_number(stack, |a| a.round()),
        "truncate" | "cvi" => ps_unary_number(stack, |a| a.trunc()),
        "cvr" => ps_unary_number(stack, |a| a),
        "eq" => ps_cmp(stack, |a, b| (a - b).abs() < f64::EPSILON),
        "ne" => ps_cmp(stack, |a, b| (a - b).abs() >= f64::EPSILON),
        "gt" => ps_cmp(stack, |a, b| a > b),
        "ge" => ps_cmp(stack, |a, b| a >= b),
        "lt" => ps_cmp(stack, |a, b| a < b),
        "le" => ps_cmp(stack, |a, b| a <= b),
        "and" => ps_logic_or_bitwise(stack, |a, b| a && b, |a, b| a & b),
        "or" => ps_logic_or_bitwise(stack, |a, b| a || b, |a, b| a | b),
        "xor" => ps_logic_or_bitwise(stack, |a, b| a ^ b, |a, b| a ^ b),
        "not" => {
            if let Some(value) = stack.pop() {
                match value {
                    PsValue::Bool(v) => stack.push(PsValue::Bool(!v)),
                    PsValue::Number(v) => stack.push(PsValue::Number((!(v as i32)) as f64)),
                    PsValue::Procedure(proc) => stack.push(PsValue::Procedure(proc)),
                }
            }
        }
        "bitshift" => ps_bin_number(stack, |a, b| {
            let value = a as i32;
            let shift = b as i32;
            if shift >= 0 {
                value.wrapping_shl(shift as u32) as f64
            } else {
                value.wrapping_shr((-shift) as u32) as f64
            }
        }),
        _ => {}
    }
}

fn pop_usize(stack: &mut Vec<PsValue>) -> Option<usize> {
    Some(stack.pop()?.as_number() as usize)
}

fn ps_unary_number(stack: &mut Vec<PsValue>, op: impl Fn(f64) -> f64) {
    if let Some(value) = stack.pop() {
        stack.push(PsValue::Number(op(value.as_number())));
    }
}

fn ps_bin_number(stack: &mut Vec<PsValue>, op: impl Fn(f64, f64) -> f64) {
    if stack.len() >= 2 {
        let b = stack.pop().unwrap().as_number();
        let a = stack.pop().unwrap().as_number();
        stack.push(PsValue::Number(op(a, b)));
    }
}

fn ps_cmp(stack: &mut Vec<PsValue>, op: impl Fn(f64, f64) -> bool) {
    if stack.len() >= 2 {
        let b = stack.pop().unwrap().as_number();
        let a = stack.pop().unwrap().as_number();
        stack.push(PsValue::Bool(op(a, b)));
    }
}

fn ps_logic_or_bitwise(
    stack: &mut Vec<PsValue>,
    bool_op: impl Fn(bool, bool) -> bool,
    int_op: impl Fn(i32, i32) -> i32,
) {
    if stack.len() < 2 {
        return;
    }

    let b = stack.pop().unwrap();
    let a = stack.pop().unwrap();
    match (&a, &b) {
        (PsValue::Bool(_), _) | (_, PsValue::Bool(_)) => {
            stack.push(PsValue::Bool(bool_op(a.as_bool(), b.as_bool())));
        }
        _ => stack.push(PsValue::Number(
            int_op(a.as_number() as i32, b.as_number() as i32) as f64,
        )),
    }
}

impl Shading {
    /// Sample the shading at position (x, y) in shading space.
    /// Returns RGBA bytes (r, g, b, a) or None if outside the shading domain.
    pub fn sample(&self, x: f64, y: f64) -> Option<(u8, u8, u8, u8)> {
        // Check bbox
        if let Some([bx0, by0, bx1, by1]) = self.bbox {
            if x < bx0 || x > bx1 || y < by0 || y > by1 {
                return self.background_rgba();
            }
        }

        if let ShadingKind::FunctionBased { domain, function } = &self.kind {
            if x < domain[0] || x > domain[1] || y < domain[2] || y > domain[3] {
                return self.background_rgba();
            }
            let components = function.eval_inputs(&[x, y]);
            return Some(self.color_space.to_rgba(&components));
        }

        if let ShadingKind::TriangleMesh {
            triangles,
            function,
        } = &self.kind
        {
            for triangle in triangles {
                if let Some(mut components) = triangle.sample(x, y) {
                    if let Some(function) = function {
                        let t = components.first().copied().unwrap_or(0.0);
                        components = function.eval(t);
                    }
                    return Some(self.color_space.to_rgba(&components));
                }
            }
            return self.background_rgba();
        }

        let t = match &self.kind {
            ShadingKind::Axial {
                x0,
                y0,
                x1,
                y1,
                t0,
                t1,
                extend_start,
                extend_end,
                ..
            } => {
                let dx = x1 - x0;
                let dy = y1 - y0;
                let len_sq = dx * dx + dy * dy;
                if len_sq < 1e-12 {
                    return None;
                }
                let t_raw = ((x - x0) * dx + (y - y0) * dy) / len_sq;
                // Map [0,1] → [t0,t1]
                let t_mapped = t0 + t_raw * (t1 - t0);

                // Clamp based on extend flags
                if t_raw < 0.0 && !extend_start {
                    return self.background_rgba();
                }
                if t_raw > 1.0 && !extend_end {
                    return self.background_rgba();
                }
                t_mapped.clamp((*t0).min(*t1), (*t0).max(*t1))
            }
            ShadingKind::Radial {
                x0,
                y0,
                r0,
                x1,
                y1,
                r1,
                t0,
                t1,
                extend_start,
                extend_end,
                ..
            } => {
                // Solve for parameter t where point (x,y) lies on the circle at t.
                // Circle(t) has center (x0 + t*(x1-x0), y0 + t*(y1-y0)) and
                // radius r0 + t*(r1-r0).
                //
                // (x - cx(t))^2 + (y - cy(t))^2 = r(t)^2
                //
                // This is a quadratic in t: a*t^2 + b*t + c = 0
                let dx = x1 - x0;
                let dy = y1 - y0;
                let dr = r1 - r0;

                let a = dx * dx + dy * dy - dr * dr;
                let b = 2.0 * (dx * (x0 - x) + dy * (y0 - y) - dr * r0);
                let c = (x0 - x) * (x0 - x) + (y0 - y) * (y0 - y) - r0 * r0;

                let t_raw = if a.abs() < 1e-12 {
                    // Linear case (concentric circles or equal radii)
                    if b.abs() < 1e-12 {
                        return self.background_rgba();
                    }
                    let t = -c / b;
                    let radius = r0 + dr * t;
                    if radius < 0.0 {
                        return self.background_rgba();
                    }
                    t
                } else {
                    let disc = b * b - 4.0 * a * c;
                    if disc < 0.0 {
                        return self.background_rgba();
                    }
                    let sq = disc.sqrt();
                    let t_a = (-b - sq) / (2.0 * a);
                    let t_b = (-b + sq) / (2.0 * a);
                    let min_t = t_a.min(t_b);
                    let max_t = t_a.max(t_b);

                    let valid_min = !((min_t < 0.0 && !extend_start) || (r0 + dr * min_t) < 0.0);
                    let valid_max = !((max_t > 1.0 && !extend_end) || (r0 + dr * max_t) < 0.0);

                    match (valid_min, valid_max) {
                        (_, true) => max_t,
                        (true, false) => min_t,
                        (false, false) => return self.background_rgba(),
                    }
                };

                let t_mapped = t0 + t_raw * (t1 - t0);

                if t_raw < 0.0 && !extend_start {
                    return self.background_rgba();
                }
                if t_raw > 1.0 && !extend_end {
                    return self.background_rgba();
                }
                t_mapped.clamp((*t0).min(*t1), (*t0).max(*t1))
            }
            ShadingKind::FunctionBased { .. } => unreachable!(),
            ShadingKind::TriangleMesh { .. } => unreachable!(),
        };

        // Normalize t to [0,1] for function evaluation
        let function = match &self.kind {
            ShadingKind::Axial { function, .. } | ShadingKind::Radial { function, .. } => function,
            ShadingKind::FunctionBased { function, .. } => function,
            ShadingKind::TriangleMesh { .. } => unreachable!(),
        };

        let components = function.eval(t);
        Some(self.color_space.to_rgba(&components))
    }

    pub fn uses_pixel_center_sampling(&self) -> bool {
        match &self.kind {
            ShadingKind::FunctionBased { .. } => true,
            ShadingKind::Axial {
                x0,
                y0,
                x1,
                y1,
                extend_start,
                extend_end,
                ..
            } => {
                // Match hayro's shading texture rasterization for diagonal
                // axial shadings with symmetric extend behavior, where texels
                // are sampled at pixel centers (`Affine::translate((0.5, 0.5))`).
                // Keep edge sampling for horizontal/vertical and asymmetric
                // one-sided extend cases, which preserves existing mutool
                // golden matches.
                let diagonal = (x1 - x0).abs() > 1e-9 && (y1 - y0).abs() > 1e-9;
                diagonal && (*extend_start == *extend_end)
            }
            ShadingKind::Radial { .. } | ShadingKind::TriangleMesh { .. } => true,
        }
    }

    fn background_rgba(&self) -> Option<(u8, u8, u8, u8)> {
        self.background
            .as_ref()
            .map(|components| self.color_space.to_rgba(components))
    }
}

pub(super) fn calrgb_to_rgb(
    components: &[f64],
    white_point: &[f64; 3],
    black_point: &[f64; 3],
    gamma: &[f64; 3],
    matrix: &[f64; 9],
) -> [f64; 3] {
    let input = [
        components.first().copied().unwrap_or(0.0).clamp(0.0, 1.0),
        components.get(1).copied().unwrap_or(0.0).clamp(0.0, 1.0),
        components.get(2).copied().unwrap_or(0.0).clamp(0.0, 1.0),
    ];
    let agr = if input[0] == 1.0 {
        1.0
    } else {
        input[0].powf(gamma[0])
    };
    let bgg = if input[1] == 1.0 {
        1.0
    } else {
        input[1].powf(gamma[1])
    };
    let cgb = if input[2] == 1.0 {
        1.0
    } else {
        input[2].powf(gamma[2])
    };

    let xyz = [
        matrix[0] * agr + matrix[3] * bgg + matrix[6] * cgb,
        matrix[1] * agr + matrix[4] * bgg + matrix[7] * cgb,
        matrix[2] * agr + matrix[5] * bgg + matrix[8] * cgb,
    ];
    let xyz_flat = normalize_white_point_to_flat(white_point, &xyz);
    let xyz_black = compensate_black_point(black_point, &xyz_flat);
    let xyz_d65 = normalize_white_point_to_d65(&[1.0, 1.0, 1.0], &xyz_black);
    let srgb = matrix_product(&SRGB_D65_XYZ_TO_RGB_MATRIX, &xyz_d65);
    [
        srgb_transfer_function(srgb[0]),
        srgb_transfer_function(srgb[1]),
        srgb_transfer_function(srgb[2]),
    ]
}

const BRADFORD_SCALE_MATRIX: [f64; 9] = [
    0.8951, 0.2664, -0.1614, -0.7502, 1.7135, 0.0367, 0.0389, -0.0685, 1.0296,
];
const BRADFORD_SCALE_INVERSE_MATRIX: [f64; 9] = [
    0.9869929, -0.1470543, 0.1599627, 0.4323053, 0.5183603, 0.0492912, -0.0085287, 0.0400428,
    0.9684867,
];
const SRGB_D65_XYZ_TO_RGB_MATRIX: [f64; 9] = [
    3.2404542, -1.5371385, -0.4985314, -0.969266, 1.8760108, 0.0415560, 0.0556434, -0.2040259,
    1.0572252,
];
const D65_WHITEPOINT: [f64; 3] = [0.95047, 1.0, 1.08883];

fn srgb_transfer_function(color: f64) -> f64 {
    if color <= 0.0031308 {
        (12.92 * color).clamp(0.0, 1.0)
    } else if color >= 0.99554525 {
        1.0
    } else {
        ((1.0 + 0.055) * color.powf(1.0 / 2.4) - 0.055).clamp(0.0, 1.0)
    }
}

fn matrix_product(a: &[f64; 9], b: &[f64; 3]) -> [f64; 3] {
    [
        a[0] * b[0] + a[1] * b[1] + a[2] * b[2],
        a[3] * b[0] + a[4] * b[1] + a[5] * b[2],
        a[6] * b[0] + a[7] * b[1] + a[8] * b[2],
    ]
}

fn normalize_white_point_to_flat(source_white_point: &[f64; 3], xyz: &[f64; 3]) -> [f64; 3] {
    if source_white_point[0] == 1.0 && source_white_point[2] == 1.0 {
        return *xyz;
    }
    let lms = matrix_product(&BRADFORD_SCALE_MATRIX, xyz);
    let lms_flat = [
        lms[0] / source_white_point[0],
        lms[1] / source_white_point[1],
        lms[2] / source_white_point[2],
    ];
    matrix_product(&BRADFORD_SCALE_INVERSE_MATRIX, &lms_flat)
}

fn normalize_white_point_to_d65(source_white_point: &[f64; 3], xyz: &[f64; 3]) -> [f64; 3] {
    let lms = matrix_product(&BRADFORD_SCALE_MATRIX, xyz);
    let lms_d65 = [
        lms[0] * D65_WHITEPOINT[0] / source_white_point[0],
        lms[1] * D65_WHITEPOINT[1] / source_white_point[1],
        lms[2] * D65_WHITEPOINT[2] / source_white_point[2],
    ];
    matrix_product(&BRADFORD_SCALE_INVERSE_MATRIX, &lms_d65)
}

fn compensate_black_point(source_bp: &[f64; 3], xyz_flat: &[f64; 3]) -> [f64; 3] {
    if source_bp == &[0.0, 0.0, 0.0] {
        return *xyz_flat;
    }

    let zero_decode_l = decode_l(0.0);
    let mut out = [0.0; 3];
    for i in 0..3 {
        let src = decode_l(source_bp[i]);
        let scale = (1.0 - zero_decode_l) / (1.0 - src);
        let offset = 1.0 - scale;
        out[i] = xyz_flat[i] * scale + offset;
    }
    out
}

fn decode_l(l: f64) -> f64 {
    if l < 0.0 {
        -decode_l(-l)
    } else if l > 8.0 {
        ((l + 16.0) / 116.0).powi(3)
    } else {
        l * ((8.0_f64 + 16.0) / 116.0).powi(3) / 8.0
    }
}

#[cfg(test)]
mod tests {
    use super::{calrgb_to_rgb, ShadingFunction};

    fn reflected_gray_function() -> ShadingFunction {
        ShadingFunction::PostScript {
            program: br#"{pop
abs dup truncate dup cvi 2 mod 0 gt 3 1 roll sub exch {1 exch sub} if
dup 0 le {pop 1 1 1 0} if
dup dup 0 gt exch 1 le and {
0 sub dup -1 mul 1 add exch dup -1 mul 1 add exch -1 mul 1 add
0} if
0 gt {0 0 0} if
}"#
            .to_vec(),
            domain: vec![-2.6336926, 3.6249996, -2.0646263, 7.056975],
            range: vec![0.0, 1.0, 0.0, 1.0, 0.0, 1.0],
        }
    }

    #[test]
    fn type4_reflected_gray_repeats_over_x() {
        let function = reflected_gray_function();
        let samples = [0.0, 0.5, 1.0, 1.5, 2.0].map(|x| function.eval_inputs(&[x, 0.0]));
        let expected = [1.0, 0.5, 0.0, 0.5, 1.0];

        for (components, expected_gray) in samples.iter().zip(expected) {
            assert_eq!(components.len(), 3);
            for component in components {
                assert!((component - expected_gray).abs() < 1e-6);
            }
        }
    }

    #[test]
    fn sampled_function_interpolates_and_decodes() {
        let function = ShadingFunction::Sampled {
            domain: vec![0.0, 1.0],
            range: vec![0.0, 20.0],
            size: vec![2],
            bits_per_sample: 8,
            encode: vec![0.0, 1.0],
            decode: vec![10.0, 20.0],
            samples: vec![0.0, 1.0],
        };

        let output = function.eval(0.5);

        assert_eq!(output.len(), 1);
        assert!((output[0] - 15.0).abs() < 1e-6);
    }

    /// CalRGB: black input always produces near-zero output.
    #[test]
    fn calrgb_black_is_near_zero() {
        let wp = [1.0, 1.0, 1.0];
        let bp = [0.0, 0.0, 0.0];
        let gamma = [1.0, 1.0, 1.0];
        let matrix = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];

        let black = calrgb_to_rgb(&[0.0, 0.0, 0.0], &wp, &bp, &gamma, &matrix);
        assert!(
            black.iter().all(|&v| v < 0.01),
            "CalRGB black should be near-zero: {:?}",
            black
        );
    }

    /// CalRGB: higher input produces higher output (monotonicity).
    #[test]
    fn calrgb_monotonicity() {
        let wp = [1.0, 1.0, 1.0];
        let bp = [0.0, 0.0, 0.0];
        let gamma = [1.0, 1.0, 1.0];
        let matrix = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];

        let lo = calrgb_to_rgb(&[0.25, 0.25, 0.25], &wp, &bp, &gamma, &matrix);
        let hi = calrgb_to_rgb(&[0.75, 0.75, 0.75], &wp, &bp, &gamma, &matrix);
        // With identity matrix the Bradford normalization to D65 produces
        // values < 1 for achromatic inputs, so hi > lo.
        assert!(hi[0] > lo[0], "CalRGB should be monotonic: lo={:?} hi={:?}", lo, hi);
        assert!(hi[1] > lo[1], "CalRGB should be monotonic");
        assert!(hi[2] > lo[2], "CalRGB should be monotonic");
    }

    /// CalRGB with [1,1,1] white point and non-unity gamma.
    /// Gamma < 1 brightens; gamma > 1 darkens.
    #[test]
    fn calrgb_gamma_darkens() {
        let wp = [1.0, 1.0, 1.0];
        let bp = [0.0, 0.0, 0.0];
        let gamma = [2.0, 2.0, 2.0]; // square -> darkens mid-tones
        let matrix = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];

        let mid_linear = calrgb_to_rgb(&[0.5, 0.5, 0.5], &wp, &bp, &gamma, &matrix);
        let mid_gamma1 = calrgb_to_rgb(&[0.5, 0.5, 0.5], &wp, &bp, &[1.0, 1.0, 1.0], &matrix);
        // gamma=2 makes 0.5^2=0.25 which is darker than gamma=1's 0.5
        assert!(mid_linear[0] < mid_gamma1[0], "gamma=2 should darken vs gamma=1");
    }
}
