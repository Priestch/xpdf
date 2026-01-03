# PDF-X Tauri App - Short-Term Implementation Plan

## Status
- ✅ Workspace restructured (pdf-x-core, cli, tauri-app)
- ✅ CLI still functional
- ✅ Tauri backend structure created
- ⚠️ **Compilation error**: Tauri 2.0 State API needs fixing
- ⏳ React frontend not started

## Short-Term Focus

**Goal**: Get a minimal working Tauri app that can open a PDF and display metadata

### 1. Fix Backend Compilation (Next)
- Fix Tauri 2.0 State access API
- Use `state.inner()` or proper dereferencing
- Test commands compile successfully

### 2. Create Minimal Frontend
- Initialize React + Vite in `tauri-app/ui/`
- Create basic App.jsx with file open dialog
- Test Tauri dev server works

### 3. Connect Frontend to Backend
- Implement `useDocument` hook
- Wire up `open_pdf_file` command
- Display basic metadata (title, page count, file size)

### 4. Add MVP Features (Iterative)
- Metadata panel component
- Outline/bookmarks navigation
- Basic page viewer (placeholder)

### 5. Polish
- Error handling
- Loading states
- Basic styling

## Implementation Philosophy

**Iterative approach**: Implement features as needed, not everything upfront.

**Get it working first**: Prioritize functionality over perfection.

**Test as we go**: After each step, verify it actually works.

## Current Blocker

State access in commands.rs needs to use proper Tauri 2.0 API:
```rust
// Current (broken):
let guard = state.document.lock().unwrap();

// Should be:
let guard = state.inner().document.lock().unwrap();
// OR
let guard = (*state).document.lock().unwrap();
```

## Next Steps

1. Fix State API (15 min)
2. Test `cargo check -p pdf-x-app` (5 min)
3. Create minimal UI (30 min)
4. Test basic functionality (15 min)

**Target**: Have a working Tauri app that opens PDFs within 2 hours.
