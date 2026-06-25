import React, { useState, useEffect, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';

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
  const [devicePixelRatio, setDevicePixelRatio] = useState(
    Math.max(1, window.devicePixelRatio || 1)
  );
  const renderRequestRef = useRef(0);
  const renderScale = computeRenderScale(zoomPercent, devicePixelRatio);

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
  }, [currentPage, documentLoaded, metadata, textTraceEnabled, renderScale]);

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
          <div className="page-render">
            <img
              src={`data:image/png;base64,${renderedPage.image_data}`}
              alt={`Page ${currentPage + 1}`}
              className="rendered-page-image"
              style={{
                width: `${imageWidth}px`,
                height: `${imageHeight}px`,
              }}
            />
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
