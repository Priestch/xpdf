import React, { useState, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';

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

  // Render the current page when page index or document changes
  useEffect(() => {
    if (!documentLoaded || !metadata) {
      setRenderedPage(null);
      return;
    }

    const renderCurrentPage = async () => {
      setIsLoading(true);
      setError(null);
      try {
        console.log('Rendering page:', currentPage);
        const result = await invoke('render_page', {
          pageIndex: currentPage,
          scale: 1.5, // Scale for better display quality
        });
        console.log('Rendered page result:', result);
        setRenderedPage(result);
      } catch (err) {
        console.error('Error rendering page:', err);
        setError(err.toString());
        setRenderedPage(null);
      } finally {
        setIsLoading(false);
      }
    };

    renderCurrentPage();
  }, [currentPage, documentLoaded, metadata]);

  const handlePageChange = (newPage) => {
    if (newPage >= 0 && newPage < (metadata?.page_count || 0)) {
      setCurrentPage(newPage);
    }
  };

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
