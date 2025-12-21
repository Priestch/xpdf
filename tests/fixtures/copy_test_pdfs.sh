#!/bin/bash
# Script to copy essential test PDFs from PDF.js submodule

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PDF_JS_DIR="$SCRIPT_DIR/../../pdf.js/test/pdfs"
TARGET_DIR="$SCRIPT_DIR/pdfs"

# Array of essential test PDFs to copy
TEST_PDFS=(
    "basicapi.pdf"
    "tracemonkey.pdf"
    "empty.pdf"
    "rotation.pdf"
    "asciihexdecode.pdf"
    "simpletype3font.pdf"
    "TrueType_without_cmap.pdf"
    "annotation-border-styles.pdf"
)

# Create target directory
mkdir -p "$TARGET_DIR"

echo "Copying essential test PDFs from PDF.js..."
echo "Source: $PDF_JS_DIR"
echo "Target: $TARGET_DIR"
echo ""

copied=0
missing=0

for pdf in "${TEST_PDFS[@]}"; do
    if [ -f "$PDF_JS_DIR/$pdf" ]; then
        cp "$PDF_JS_DIR/$pdf" "$TARGET_DIR/"
        echo "✓ Copied: $pdf"
        ((copied++))
    else
        echo "✗ Missing: $pdf"
        ((missing++))
    fi
done

echo ""
echo "Summary: $copied copied, $missing missing"

# Create placeholder files for PDFs we'll need to create or find
echo ""
echo "Creating placeholder files for custom test PDFs..."

# Create a simple .link file for large document testing
cat > "$TARGET_DIR/large-document.pdf.link" << 'EOF'
# This is a placeholder for a large PDF document
# Replace with actual URL or create a large test PDF
# Example: https://example.com/large-document.pdf
EOF

# Create placeholder markers for PDFs we need
for placeholder in "xref-stream.pdf" "linearized.pdf" "compressed-object-stream.pdf" "flatedecode.pdf" "bad-xref.pdf"; do
    if [ ! -f "$TARGET_DIR/$placeholder" ]; then
        echo "NOTE: Need to create or find: $placeholder" >> "$TARGET_DIR/README.txt"
    fi
done

echo ""
echo "Done! Check $TARGET_DIR/README.txt for PDFs that need to be created."
