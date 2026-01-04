import React, { useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { open } from '@tauri-apps/plugin-dialog';
import OutlineViewer from './components/OutlineViewer';
import MainPanel from './components/MainPanel';
import InspectorPanel from './components/InspectorPanel';

function App() {
  const [metadata, setMetadata] = useState(null);
  const [error, setError] = useState(null);
  const [isLoading, setIsLoading] = useState(false);
  const [viewMode, setViewMode] = useState('inspector'); // 'inspector' or 'viewer'

  const handleOpenFile = async () => {
    try {
      // Open native file picker dialog
      const selected = await open({
        multiple: false,
        filters: [{
          name: 'PDF',
          extensions: ['pdf']
        }]
      });

      if (!selected) {
        return; // User cancelled
      }

      setError(null);
      setIsLoading(true);

      console.log('Opening file:', selected);
      const meta = await invoke('open_pdf_file', { filePath: selected });
      console.log('Metadata:', meta);
      setMetadata(meta);

      // Reset to inspector mode when opening new file
      setViewMode('inspector');
    } catch (err) {
      console.error('Error:', err);
      setError(err.toString());
    } finally {
      setIsLoading(false);
    }
  };

  const handleClose = async () => {
    try {
      await invoke('close_document');
      setMetadata(null);
      setError(null);
      setViewMode('inspector');
    } catch (err) {
      console.error('Error:', err);
      setError(err.toString());
    }
  };

  const toggleViewMode = () => {
    setViewMode(viewMode === 'inspector' ? 'viewer' : 'inspector');
  };

  return (
    <div className="app">
      <header className="app-header">
        <h1>📄 PDF-X Inspector</h1>
        <div className="header-center">
          {metadata && (
            <div className="view-mode-toggle">
              <button
                className={viewMode === 'inspector' ? 'active' : ''}
                onClick={() => setViewMode('inspector')}
              >
                🔍 Inspector
              </button>
              <button
                className={viewMode === 'viewer' ? 'active' : ''}
                onClick={() => setViewMode('viewer')}
              >
                📄 Page Viewer
              </button>
            </div>
          )}
        </div>
        <div className="header-actions">
          {metadata && (
            <button onClick={handleClose} className="close-button">
              Close
            </button>
          )}
          <button onClick={handleOpenFile} disabled={isLoading}>
            {isLoading ? 'Loading...' : metadata ? 'Open New PDF' : 'Open PDF'}
          </button>
        </div>
      </header>

      {error && (
        <div className="error-message">
          Error: {error}
        </div>
      )}

      <main className="app-main">
        {/* Show bookmarks sidebar in both modes if document is loaded */}
        {metadata && viewMode === 'viewer' && (
          <aside className="sidebar bookmarks-panel">
            <OutlineViewer documentLoaded={!!metadata} />
          </aside>
        )}

        {/* Main content area */}
        {metadata ? (
          <>
            {viewMode === 'inspector' ? (
              <>
                {/* Inspector Mode: Show inspector panel in center */}
                <div className="main-content inspector-mode">
                  <InspectorPanel documentLoaded={!!metadata} metadata={metadata} />
                </div>

                {/* Optional bookmarks in inspector mode */}
                <aside className="sidebar bookmarks-panel">
                  <OutlineViewer documentLoaded={!!metadata} />
                </aside>
              </>
            ) : (
              <>
                {/* Viewer Mode: Show page viewer in center */}
                <div className="main-content viewer-mode">
                  <MainPanel documentLoaded={!!metadata} metadata={metadata} />
                </div>
              </>
            )}
          </>
        ) : (
          <>
            {/* No document loaded - show welcome in center */}
            <div className="main-content">
              <MainPanel documentLoaded={false} metadata={null} />
            </div>
          </>
        )}
      </main>
    </div>
  );
}

export default App;
