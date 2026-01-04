import React, { useState } from 'react';

/**
 * MainPanel - Central panel showing the main content
 *
 * Switches between different views:
 * - Page Viewer (PDF rendering)
 * - Welcome screen
 */
function MainPanel({ documentLoaded, metadata }) {
  const [currentPage, setCurrentPage] = useState(0);

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
          </div>
        )}
        <div className="page-controls">
          <button
            onClick={() => setCurrentPage(Math.max(0, currentPage - 1))}
            disabled={currentPage === 0}
          >
            ← Previous
          </button>
          <button
            onClick={() => setCurrentPage(Math.min((metadata?.page_count || 1) - 1, currentPage + 1))}
            disabled={currentPage >= (metadata?.page_count || 1) - 1}
          >
            Next →
          </button>
        </div>
      </div>

      <div className="panel-content page-viewer">
        <div className="page-placeholder">
          <div className="placeholder-content">
            <h3>Page {currentPage + 1}</h3>
            <p>PDF rendering will be implemented here</p>
            <p className="hint">
              For now, use the Inspector panel (right) to view:
            </p>
            <ul>
              <li>Document metadata</li>
              <li>Page sizes</li>
              <li>Bookmarks (left panel)</li>
            </ul>
          </div>
        </div>
      </div>
    </div>
  );
}

export default MainPanel;
