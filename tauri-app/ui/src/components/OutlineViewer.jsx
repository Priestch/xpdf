import React, { useState, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';

/**
 * OutlineViewer component - displays PDF bookmarks/outline
 *
 * Features:
 * - Hierarchical tree structure with expand/collapse
 * - Visual styling (bold, italic, custom colors)
 * - Page number display
 * - URL link support
 */
function OutlineViewer({ documentLoaded }) {
  const [outline, setOutline] = useState(null);
  const [error, setError] = useState(null);
  const [isLoading, setIsLoading] = useState(false);

  // Track expanded items using a Set of unique keys
  const [expandedItems, setExpandedItems] = useState(new Set());

  useEffect(() => {
    if (documentLoaded) {
      loadOutline();
    } else {
      setOutline(null);
      setError(null);
      setExpandedItems(new Set());
    }
  }, [documentLoaded]);

  const loadOutline = async () => {
    try {
      setIsLoading(true);
      setError(null);
      const data = await invoke('get_document_outline');
      setOutline(data);

      // Auto-expand first level items
      if (data && data.length > 0) {
        setExpandedItems(new Set(data.map((_, index) => `0-${index}`)));
      }
    } catch (err) {
      console.error('Failed to load outline:', err);
      setError(err.toString());
    } finally {
      setIsLoading(false);
    }
  };

  const toggleExpand = (key) => {
    const newExpanded = new Set(expandedItems);
    if (newExpanded.has(key)) {
      newExpanded.delete(key);
    } else {
      newExpanded.add(key);
    }
    setExpandedItems(newExpanded);
  };

  const handleItemClick = (item) => {
    if (item.url) {
      // Open URL in browser
      window.open(item.url, '_blank');
    } else if (item.page !== null && item.page !== undefined) {
      // Navigate to page (page viewer integration needed)
      console.log('Navigate to page:', item.page);
      // TODO: Integrate with page viewer when implemented
    }
  };

  const renderOutlineItem = (item, key, level = 0) => {
    const hasChildren = item.children && item.children.length > 0;
    const isExpanded = expandedItems.has(key);
    const hasCount = item.count !== undefined && item.count !== null;

    return (
      <div key={key} className="outline-item">
        <div
          className="outline-item-header"
          style={{ paddingLeft: `${level * 16 + 8}px` }}
          onClick={() => handleItemClick(item)}
        >
          {hasChildren && (
            <span
              className="outline-toggle"
              onClick={(e) => {
                e.stopPropagation();
                toggleExpand(key);
              }}
            >
              {isExpanded ? '▼' : hasCount && item.count < 0 ? '+' : '▶'}
            </span>
          )}
          {!hasChildren && <span className="outline-toggle-spacer" />}
          <span
            className={`outline-title ${item.bold ? 'bold' : ''} ${item.italic ? 'italic' : ''}`}
            style={{
              color: item.color ? `rgb(${item.color.join(',')})` : 'inherit',
            }}
          >
            {item.title}
          </span>
          {item.page !== null && item.page !== undefined && (
            <span className="outline-page">p. {item.page + 1}</span>
          )}
          {item.dest_type && (
            <span className="outline-dest-type">{item.dest_type}</span>
          )}
        </div>
        {hasChildren && isExpanded && (
          <div className="outline-children">
            {item.children.map((child, childIndex) =>
              renderOutlineItem(child, `${key}-${childIndex}`, level + 1)
            )}
          </div>
        )}
      </div>
    );
  };

  if (!documentLoaded) {
    return null;
  }

  if (isLoading) {
    return (
      <div className="outline-viewer">
        <h3>Bookmarks</h3>
        <div className="outline-loading">Loading bookmarks...</div>
      </div>
    );
  }

  if (error) {
    return (
      <div className="outline-viewer">
        <h3>Bookmarks</h3>
        <div className="outline-error">Error: {error}</div>
      </div>
    );
  }

  if (!outline) {
    return null;
  }

  if (outline.length === 0) {
    return (
      <div className="outline-viewer">
        <h3>Bookmarks</h3>
        <div className="outline-empty">This document has no bookmarks</div>
      </div>
    );
  }

  return (
    <div className="outline-viewer">
      <h3>Bookmarks</h3>
      <div className="outline-list">
        {outline.map((item, index) => renderOutlineItem(item, `0-${index}`))}
      </div>
    </div>
  );
}

export default OutlineViewer;
