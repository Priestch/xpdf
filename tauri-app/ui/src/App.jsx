import React, { useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { open } from '@tauri-apps/plugin-dialog';

function App() {
  const [metadata, setMetadata] = useState(null);
  const [error, setError] = useState(null);
  const [isLoading, setIsLoading] = useState(false);

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
    } catch (err) {
      console.error('Error:', err);
      setError(err.toString());
    } finally {
      setIsLoading(false);
    }
  };

  const formatFileSize = (bytes) => {
    if (!bytes) return 'Unknown';
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(2)} KB`;
    return `${(bytes / (1024 * 1024)).toFixed(2)} MB`;
  };

  return (
    <div className="app">
      <header className="app-header">
        <h1>📄 PDF-X Viewer</h1>
        <button onClick={handleOpenFile} disabled={isLoading}>
          {isLoading ? 'Loading...' : 'Open PDF'}
        </button>
      </header>

      {error && (
        <div className="error-message">
          Error: {error}
        </div>
      )}

      {metadata && (
        <main className="app-main">
          <section className="metadata-section">
            <h2>Document Properties</h2>
            <div className="metadata-grid">
              {metadata.title && (
                <div className="metadata-item">
                  <span className="label">Title:</span>
                  <span className="value">{metadata.title}</span>
                </div>
              )}
              {metadata.author && (
                <div className="metadata-item">
                  <span className="label">Author:</span>
                  <span className="value">{metadata.author}</span>
                </div>
              )}
              <div className="metadata-item">
                <span className="label">Pages:</span>
                <span className="value">{metadata.page_count}</span>
              </div>
              <div className="metadata-item">
                <span className="label">File Size:</span>
                <span className="value">{formatFileSize(metadata.file_size)}</span>
              </div>
              <div className="metadata-item">
                <span className="label">PDF Version:</span>
                <span className="value">{metadata.pdf_version}</span>
              </div>
              <div className="metadata-item">
                <span className="label">Linearized:</span>
                <span className="value">{metadata.is_linearized ? 'Yes' : 'No'}</span>
              </div>
              {metadata.creator && (
                <div className="metadata-item">
                  <span className="label">Creator:</span>
                  <span className="value">{metadata.creator}</span>
                </div>
              )}
              {metadata.producer && (
                <div className="metadata-item">
                  <span className="label">Producer:</span>
                  <span className="value">{metadata.producer}</span>
                </div>
              )}
            </div>
          </section>

          <section className="viewer-section">
            <h2>Page Viewer</h2>
            <p>Page viewer coming soon...</p>
          </section>
        </main>
      )}

      {!metadata && !isLoading && (
        <main className="welcome-screen">
          <h2>Welcome to PDF-X Viewer</h2>
          <p>Click "Open PDF" to inspect a PDF file</p>
        </main>
      )}
    </div>
  );
}

export default App;
