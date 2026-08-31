import React, { useState, useEffect, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { save } from '@tauri-apps/plugin-dialog';

const MIN_ZOOM = 50;
const MAX_ZOOM = 300;
const ZOOM_STEP = 25;
const MAX_RENDER_SCALE = 8.0;

function clamp(value, min, max) {
  return Math.min(max, Math.max(min, value));
}

function computeRenderScale(zoomPercent, devicePixelRatio) {
  const zoomFactor = zoomPercent / 100;
  // Use an integer DPR bucket to avoid fractional downscaling blur/ghosting
  // that can make glyphs look overlapped on some displays (e.g. 125% scaling).
  const effectiveDpr = Math.max(1, Math.round(devicePixelRatio));
  return clamp(zoomFactor * effectiveDpr, 1.0, MAX_RENDER_SCALE);
}

/**
 * MainPanel - Central panel showing the main content
 *
 * Switches between different views:
 * - Page Viewer (PDF rendering)
 * - Welcome screen
 */
function MainPanel({ documentLoaded, metadata }) {
  const [currentPage, setCurrentPage] = useState(0);
  const [renderedPage, setRenderedPage] = useState(null);
  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState(null);
  const [textTraceEnabled, setTextTraceEnabled] = useState(false);
  const [zoomPercent, setZoomPercent] = useState(100);
  const [editorState, setEditorState] = useState({ dirty: false, can_undo: false, can_redo: false });
  const [highlightMode, setHighlightMode] = useState(false);
  const [selection, setSelection] = useState(null);
  const [renderRevision, setRenderRevision] = useState(0);
  const pageRenderRef = useRef(null);
  const [devicePixelRatio, setDevicePixelRatio] = useState(
    Math.max(1, window.devicePixelRatio || 1)
  );
  const renderRequestRef = useRef(0);
  const renderScale = computeRenderScale(zoomPercent, devicePixelRatio);

  useEffect(() => {
    setCurrentPage(0);
    setEditorState({ dirty: false, can_undo: false, can_redo: false });
    setHighlightMode(false);
    setSelection(null);
  }, [metadata]);

  useEffect(() => {
    const syncDpr = () => setDevicePixelRatio(Math.max(1, window.devicePixelRatio || 1));
    window.addEventListener('resize', syncDpr);
    return () => window.removeEventListener('resize', syncDpr);
  }, []);

  // Render the current page when page index or document changes
  useEffect(() => {
    if (!documentLoaded || !metadata) {
      setRenderedPage(null);
      return;
    }

    const renderCurrentPage = async () => {
      const requestId = ++renderRequestRef.current;
      const requestedScale = renderScale;
      setIsLoading(true);
      setError(null);
      try {
        console.log('Rendering page:', currentPage);
        const result = await invoke('render_page', {
          pageIndex: currentPage,
          scale: requestedScale,
          textTrace: textTraceEnabled,
        });
        if (requestId !== renderRequestRef.current) {
          return;
        }
        console.log('Rendered page result:', result);
        setRenderedPage({
          ...result,
          requestedScale,
        });
      } catch (err) {
        if (requestId !== renderRequestRef.current) {
          return;
        }
        console.error('Error rendering page:', err);
        setError(err.toString());
        setRenderedPage(null);
      } finally {
        if (requestId === renderRequestRef.current) {
          setIsLoading(false);
        }
      }
    };

    renderCurrentPage();
  }, [currentPage, documentLoaded, metadata, textTraceEnabled, renderScale, renderRevision]);

  const runEdit = async (command) => {
    try {
      setError(null);
      const nextState = await invoke(command);
      setEditorState(nextState);
      setRenderRevision((value) => value + 1);
    } catch (err) {
      setError(err.toString());
    }
  };

  const handleSaveAs = async () => {
    try {
      const path = await save({ filters: [{ name: 'PDF', extensions: ['pdf'] }] });
      if (!path) return;
      const nextState = await invoke('save_pdf_as', { filePath: path });
      setEditorState(nextState);
      setRenderRevision((value) => value + 1);
    } catch (err) {
      setError(err.toString());
    }
  };

  const pointerPosition = (event) => {
    const bounds = pageRenderRef.current.getBoundingClientRect();
    return {
      x: clamp(event.clientX - bounds.left, 0, bounds.width),
      y: clamp(event.clientY - bounds.top, 0, bounds.height),
      width: bounds.width,
      height: bounds.height,
    };
  };

  const handlePointerDown = (event) => {
    if (!highlightMode || !renderedPage) return;
    event.currentTarget.setPointerCapture(event.pointerId);
    const point = pointerPosition(event);
    setSelection({ startX: point.x, startY: point.y, x: point.x, y: point.y });
  };

  const handlePointerMove = (event) => {
    if (!selection) return;
    const point = pointerPosition(event);
    setSelection((current) => ({ ...current, x: point.x, y: point.y }));
  };

  const handlePointerUp = async (event) => {
    if (!selection || !renderedPage) return;
    const point = pointerPosition(event);
    const left = Math.min(selection.startX, point.x);
    const right = Math.max(selection.startX, point.x);
    const top = Math.min(selection.startY, point.y);
    const bottom = Math.max(selection.startY, point.y);
    setSelection(null);
    if (right - left < 4 || bottom - top < 4) return;
    const pageWidth = renderedPage.width / renderedPage.requestedScale;
    const pageHeight = renderedPage.height / renderedPage.requestedScale;
    const bounds = pageRenderRef.current.getBoundingClientRect();
    const rect = [
      left / bounds.width * pageWidth,
      pageHeight - bottom / bounds.height * pageHeight,
      right / bounds.width * pageWidth,
      pageHeight - top / bounds.height * pageHeight,
    ];
    try {
      const nextState = await invoke('add_highlight', {
        pageIndex: currentPage,
        rect,
        color: [1.0, 0.92, 0.2],
      });
      setEditorState(nextState);
      setRenderRevision((value) => value + 1);
    } catch (err) {
      setError(err.toString());
    }
  };

  const handlePageChange = (newPage) => {
    if (newPage >= 0 && newPage < (metadata?.page_count || 0)) {
      setCurrentPage(newPage);
    }
  };

  const handleZoomIn = () => {
    setZoomPercent((prev) => Math.min(MAX_ZOOM, prev + ZOOM_STEP));
  };

  const handleZoomOut = () => {
    setZoomPercent((prev) => Math.max(MIN_ZOOM, prev - ZOOM_STEP));
  };

  const resetZoom = () => {
    setZoomPercent(100);
  };

  const displayScale = renderedPage?.requestedScale
    ? (zoomPercent / 100) / renderedPage.requestedScale
    : 1;
  const imageWidth = renderedPage ? Math.max(1, Math.round(renderedPage.width * displayScale)) : 0;
  const imageHeight = renderedPage ? Math.max(1, Math.round(renderedPage.height * displayScale)) : 0;

  if (!documentLoaded) {
    return (
      <div className="main-panel welcome-screen">
        <div className="welcome-content">
          <h2>📄 Welcome to PDF-X Viewer</h2>
          <p>Click "Open PDF" to inspect a PDF file</p>
          <div className="features-list">
            <h3>Features:</h3>
            <ul>
              <li>📑 Bookmarks/Outlines navigation</li>
              <li>📄 Document metadata inspection</li>
              <li>📐 Page size information</li>
              <li>📝 Text extraction (coming soon)</li>
              <li>🖼️ Image extraction (coming soon)</li>
              <li>🎨 Progressive loading support</li>
            </ul>
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className="main-panel">
      <div className="panel-toolbar">
        <h2>Page Viewer</h2>
        {metadata && (
          <div className="page-info">
            <span>Page {currentPage + 1} of {metadata.page_count}</span>
            {renderedPage && (
              <span className="render-info">
                ({renderedPage.width}×{renderedPage.height}px)
              </span>
            )}
          </div>
        )}
        <div className="page-controls">
          <div className="edit-controls">
            <button
              className={highlightMode ? 'active-tool' : ''}
              onClick={() => setHighlightMode((value) => !value)}
              disabled={metadata?.is_encrypted}
              title="Highlight"
            >
              Highlight
            </button>
            <button onClick={() => runEdit('undo_edit')} disabled={!editorState.can_undo} title="Undo">↶</button>
            <button onClick={() => runEdit('redo_edit')} disabled={!editorState.can_redo} title="Redo">↷</button>
            <button onClick={handleSaveAs} disabled={!editorState.dirty} title="Save As">Save As</button>
            {editorState.dirty && <span className="dirty-indicator">Unsaved</span>}
          </div>
          <div className="zoom-controls">
            <button onClick={handleZoomOut} disabled={isLoading || zoomPercent <= MIN_ZOOM}>
              -
            </button>
            <button onClick={resetZoom} disabled={isLoading || zoomPercent === 100}>
              {zoomPercent}%
            </button>
            <button onClick={handleZoomIn} disabled={isLoading || zoomPercent >= MAX_ZOOM}>
              +
            </button>
          </div>
          <label className="trace-toggle">
            <input
              type="checkbox"
              checked={textTraceEnabled}
              onChange={(event) => setTextTraceEnabled(event.target.checked)}
              disabled={isLoading}
            />
            Trace Tw/Tc/TJ
          </label>
          <button
            onClick={() => handlePageChange(currentPage - 1)}
            disabled={currentPage === 0 || isLoading}
          >
            ← Previous
          </button>
          <button
            onClick={() => handlePageChange(currentPage + 1)}
            disabled={currentPage >= (metadata?.page_count || 1) - 1 || isLoading}
          >
            Next →
          </button>
        </div>
      </div>

      <div className="panel-content page-viewer">
        {error && (
          <div className="error-message">
            Error: {error}
          </div>
        )}

        {isLoading && (
          <div className="loading-indicator">
            <div className="spinner"></div>
            <p>Rendering page {currentPage + 1}...</p>
          </div>
        )}

        {!isLoading && renderedPage && (
          <div
            className={`page-render ${highlightMode ? 'highlight-mode' : ''}`}
            ref={pageRenderRef}
            onPointerDown={handlePointerDown}
            onPointerMove={handlePointerMove}
            onPointerUp={handlePointerUp}
          >
            <img
              src={`data:image/png;base64,${renderedPage.image_data}`}
              alt={`Page ${currentPage + 1}`}
              className="rendered-page-image"
              style={{
                width: `${imageWidth}px`,
                height: `${imageHeight}px`,
              }}
            />
            {selection && (
              <div className="highlight-selection" style={{
                left: `${Math.min(selection.startX, selection.x)}px`,
                top: `${Math.min(selection.startY, selection.y)}px`,
                width: `${Math.abs(selection.x - selection.startX)}px`,
                height: `${Math.abs(selection.y - selection.startY)}px`,
              }} />
            )}
          </div>
        )}

        {!isLoading && !renderedPage && !error && (
          <div className="page-placeholder">
            <div className="placeholder-content">
              <h3>Page {currentPage + 1}</h3>
              <p>Rendering...</p>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}

export default MainPanel;
