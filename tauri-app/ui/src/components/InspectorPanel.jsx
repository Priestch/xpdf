import React, { useState, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';

/**
 * InspectorPanel - Right sidebar showing document information
 *
 * Always visible and shows multiple sections:
 * - Document metadata
 * - Text extraction
 * - Image extraction
 * - Page sizes
 */
function InspectorPanel({ documentLoaded, metadata: externalMetadata }) {
  const [metadata, setMetadata] = useState(null);
  const [pageSizes, setPageSizes] = useState([]);
  const [activeTab, setActiveTab] = useState('metadata');
  const [text, setText] = useState(null);
  const [images, setImages] = useState(null);
  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState(null);

  useEffect(() => {
    // Use external metadata if provided, otherwise keep internal state
    if (externalMetadata) {
      setMetadata(externalMetadata);
    }
    if (documentLoaded) {
      loadPageSizes();
    } else {
      setMetadata(null);
      setPageSizes([]);
      setText(null);
      setImages(null);
    }
  }, [documentLoaded, externalMetadata]);

  const loadMetadata = async () => {
    setActiveTab('metadata');
  };

  const loadPageSizes = async () => {
    try {
      setIsLoading(true);
      const sizes = await invoke('get_page_sizes');
      setPageSizes(sizes);
    } catch (err) {
      console.error('Failed to load page sizes:', err);
      setError(err.toString());
    } finally {
      setIsLoading(false);
    }
  };

  const extractText = async () => {
    try {
      setIsLoading(true);
      setError(null);

      // Extract text from page 0 (first page)
      const result = await invoke('extract_text_from_page', { pageIndex: 0 });

      // Format text items for display
      const formattedText = result.text_items.map(item => {
        const posInfo = item.x !== 0 || item.y !== 0 ? ` [${item.x.toFixed(1)}, ${item.y.toFixed(1)}]` : '';
        const fontInfo = item.font_size ? ` (${item.font_size.toFixed(1)}pt)` : '';
        return item.text + posInfo + fontInfo;
      }).join('\n');

      setText(formattedText || 'No text found on this page');
    } catch (err) {
      console.error('Failed to extract text:', err);
      setError(err.toString());
    } finally {
      setIsLoading(false);
    }
  };

  const extractImages = async () => {
    try {
      setIsLoading(true);
      setError(null);
      // TODO: Implement image extraction command
      setImages('Image extraction not yet implemented');
    } catch (err) {
      console.error('Failed to extract images:', err);
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
    <aside className="inspector-panel">
      <div className="inspector-tabs">
        <button
          className={activeTab === 'metadata' ? 'active' : ''}
          onClick={() => setActiveTab('metadata')}
        >
          📄 Metadata
        </button>
        <button
          className={activeTab === 'text' ? 'active' : ''}
          onClick={() => setActiveTab('text')}
        >
          📝 Text
        </button>
        <button
          className={activeTab === 'images' ? 'active' : ''}
          onClick={() => setActiveTab('images')}
        >
          🖼️ Images
        </button>
        <button
          className={activeTab === 'pages' ? 'active' : ''}
          onClick={() => setActiveTab('pages')}
        >
          📐 Pages
        </button>
      </div>

      <div className="inspector-content">
        {activeTab === 'metadata' && (
          <div className="tab-content">
            <h3>Document Properties</h3>
            {metadata ? (
              <div className="metadata-list">
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
                <div className="metadata-item">
                  <span className="label">Encrypted:</span>
                  <span className="value">
                    {metadata.is_encrypted ?
                      (metadata.requires_password ? '🔒 Yes (Password Required)' : '🔒 Yes') :
                      'No'}
                  </span>
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
            ) : (
              <p className="empty-message">No document loaded</p>
            )}
          </div>
        )}

        {activeTab === 'text' && (
          <div className="tab-content">
            <h3>Text Extraction</h3>
            <button
              onClick={extractText}
              disabled={isLoading || !documentLoaded}
              className="extract-button"
            >
              {isLoading ? 'Extracting...' : 'Extract Text'}
            </button>
            {error && <div className="error-message">{error}</div>}
            {text && (
              <div className="extracted-content">
                <pre>{text}</pre>
              </div>
            )}
          </div>
        )}

        {activeTab === 'images' && (
          <div className="tab-content">
            <h3>Image Extraction</h3>
            <button
              onClick={extractImages}
              disabled={isLoading || !documentLoaded}
              className="extract-button"
            >
              {isLoading ? 'Extracting...' : 'Extract Images'}
            </button>
            {error && <div className="error-message">{error}</div>}
            {images && (
              <div className="extracted-content">
                <p>{images}</p>
              </div>
            )}
          </div>
        )}

        {activeTab === 'pages' && (
          <div className="tab-content">
            <h3>Page Sizes</h3>
            {pageSizes.length > 0 ? (
              <div className="page-sizes-list">
                {pageSizes.map((page) => (
                  <div key={page.index} className="page-size-item">
                    <span className="page-number">Page {page.index + 1}</span>
                    <span className="page-dimensions">
                      {page.width.toFixed(1)} × {page.height.toFixed(1)} pt
                    </span>
                    {page.rotation > 0 && (
                      <span className="page-rotation">
                        ({page.rotation}° rotation)
                      </span>
                    )}
                  </div>
                ))}
              </div>
            ) : (
              <p className="empty-message">No page data available</p>
            )}
          </div>
        )}
      </div>
    </aside>
  );
}

export default InspectorPanel;
