#!/usr/bin/env python3
"""
Generate minimal test PDFs for PDF-X testing.

This script creates minimal PDF files with specific features for testing:
- xref-stream.pdf: PDF with cross-reference stream (PDF 1.5+)
- linearized.pdf: Linearized PDF for fast web view
- compressed-object-stream.pdf: PDF with compressed object streams
- flatedecode.pdf: PDF with FlateDecode compression
- annotation-text.pdf: PDF with text annotations
- bad-xref.pdf: PDF with malformed xref for error recovery
- issue3115.pdf: PDF with incremental updates
"""

import struct
import zlib
import os

def write_xref_stream_pdf():
    """Generate a minimal PDF with cross-reference stream (PDF 1.5+)"""
    # Minimal PDF 1.5 with xref stream
    pdf = b"""%PDF-1.5
1 0 obj
<< /Type /Catalog /Pages 2 0 R >>
endobj
2 0 obj
<< /Type /Pages /Kids [3 0 R] /Count 1 >>
endobj
3 0 obj
<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>
endobj
4 0 obj
<< /Length 44 >>
stream
BT
/F1 12 Tf
100 700 Td
(Test) Tj
ET
endstream
endobj
5 0 obj
<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>
endobj
"""
    # Calculate xref stream
    xref_start = len(pdf)
    # XRef stream with all objects
    xref_data = b"0000000000 00000 f \n0000000009 00000 n \n0000000058 00000 n \n0000000139 00000 n \n0000000359 00000 n \n0000000457 00000 n \n"
    xref_stream = zlib.compress(xref_data)

    # Add xref stream object
    pdf += f"6 0 obj\n<< /Type /XRef /Size 7 /Index [0 7] /Filter /FlateDecode /W [1 1 1] /Length {len(xref_stream)} >>\nstream\n".encode()
    pdf += xref_stream
    pdf += b"\nendstream\nendobj\n"

    # Trailer
    pdf += b"trailer\n<< /Size 7 /Root 1 0 R /XRef 6 0 R >>\nstartxref\n" + str(xref_start).encode() + b"\n%%EOF"

    return pdf

def write_linearized_pdf():
    """Generate a minimal linearized PDF"""
    # Linearized PDF has specific structure with /Linearized dict
    pdf = b"""%PDF-1.4
1 0 obj
<< /Linearized 1 /L 1000 /E 500 /N 1 /T 1000 /H [ 488 ] /O 2 0 R >>
endobj
2 0 obj
<< /Type /Catalog /Pages 3 0 R >>
endobj
3 0 obj
<< /Type /Pages /Kids [4 0 R] /Count 1 >>
endobj
4 0 obj
<< /Type /Page /Parent 3 0 R /MediaBox [0 0 612 792] /Contents 5 0 R >>
endobj
5 0 obj
<< /Length 44 >>
stream
BT
/F1 12 Tf
100 700 Td
(Linearized) Tj
ET
endstream
endobj
xref
0 6
0000000000 65535 f
0000000009 00000 n
0000000058 00000 n
0000000111 00000 n
0000000168 00000 n
0000000289 00000 n
trailer
<< /Size 6 /Root 2 0 R >>
startxref
389
%%EOF"""
    return pdf

def write_compressed_object_stream_pdf():
    """Generate a PDF with compressed object streams"""
    pdf = b"""%PDF-1.5
1 0 obj
<< /Type /Catalog /Pages 2 0 R >>
endobj
2 0 obj
<< /Type /Pages /Kids [4 0 R] /Count 1 >>
endobj
3 0 obj
<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>
endobj
"""
    # Object stream containing objects 4 and 5
    obj4 = b"4 0 obj<</Type/Page/Parent 2 0 R/MediaBox[0 0 612 792]/Contents 6 0 R/Resources<</Font<</F1 3 0 R>>>>>>"
    obj5 = b"5 0 obj<</Length 44>>stream\nBT/F1 12 Tf 100 700 Td (Compressed) Tj ET\nendstream endobj"

    combined = obj4 + b" " + obj5
    compressed = zlib.compress(combined)

    pdf += f"6 0 obj\n<< /Type /ObjStm /N 2 /First {len(obj4)} /Filter /FlateDecode /Length {len(compressed)} >>\nstream\n".encode()
    pdf += compressed
    pdf += b"\nendstream\nendobj\n"

    # Add xref stream
    xref_start = len(pdf)
    xref_data = b"0000000000 00000 f \n0000000009 00000 n \n0000000058 00000 n \n0000000111 00000 n \n0000000000 00000 f \n0000000000 00000 f \n0000000197 00000 n \n"
    xref_stream = zlib.compress(xref_data)

    pdf += f"7 0 obj\n<< /Type /XRef /Size 8 /Index [0 8] /Filter /FlateDecode /W [1 1 1] /Length {len(xref_stream)} >>\nstream\n".encode()
    pdf += xref_stream
    pdf += b"\nendstream\nendobj\n"

    pdf += b"trailer\n<< /Size 8 /Root 1 0 R /XRef 7 0 R >>\nstartxref\n" + str(xref_start).encode() + b"\n%%EOF"

    return pdf

def write_flatedecode_pdf():
    """Generate a PDF with FlateDecode compression"""
    content = b"BT /F1 12 Tf 100 700 Td (FlateDecode) Tj ET"
    compressed = zlib.compress(content)

    pdf = b"""%PDF-1.4
1 0 obj
<< /Type /Catalog /Pages 2 0 R >>
endobj
2 0 obj
<< /Type /Pages /Kids [3 0 R] /Count 1 >>
endobj
3 0 obj
<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>
endobj
4 0 obj
<< /Filter /FlateDecode /Length """ + str(len(compressed)).encode() + b""">>
stream
""" + compressed + b"""
endstream
endobj
5 0 obj
<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>
endobj
xref
0 6
0000000000 65535 f
0000000009 00000 n
0000000058 00000 n
0000000111 00000 n
0000000289 00000 n
0000000459 00000 n
trailer
<< /Size 6 /Root 1 0 R >>
startxref
555
%%EOF"""
    return pdf

def write_annotation_text_pdf():
    """Generate a PDF with text annotations"""
    pdf = b"""%PDF-1.4
1 0 obj
<< /Type /Catalog /Pages 2 0 R >>
endobj
2 0 obj
<< /Type /Pages /Kids [3 0 R] /Count 1 >>
endobj
3 0 obj
<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 5 0 R /Annots [4 0 R] /Resources << /Font << /F1 6 0 R >> >> >>
endobj
4 0 obj
<< /Type /Annot /Subtype /Text /Rect [100 700 200 750] /Contents (This is a text annotation) /T (Note) >>
endobj
5 0 obj
<< /Length 44 >>
stream
BT
/F1 12 Tf
100 700 Td
(Page with annotation) Tj
ET
endstream
endobj
6 0 obj
<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>
endobj
xref
0 7
0000000000 65535 f
0000000009 00000 n
0000000058 00000 n
0000000111 00000 n
0000000299 00000 n
0000000409 00000 n
0000000564 00000 n
trailer
<< /Size 7 /Root 1 0 R >>
startxref
660
%%EOF"""
    return pdf

def write_bad_xref_pdf():
    """Generate a PDF with malformed xref for error recovery testing"""
    pdf = b"""%PDF-1.4
1 0 obj
<< /Type /Catalog /Pages 2 0 R >>
endobj
2 0 obj
<< /Type /Pages /Kids [3 0 R] /Count 1 >>
endobj
3 0 obj
<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R >>
endobj
4 0 obj
<< /Length 20 >>
stream
BT
(Test) Tj
ET
endstream
endobj
xref
0 5
0000000000 65535 f
0000000009 00000 n
0000000058 00000 n
0000000111 00000 n
0000000209 00000 n
trailer
<< /Size 5 /Root 1 0 R >>
startxref
999999
%%EOF"""  # startxref points to wrong location
    return pdf

def write_incremental_update_pdf():
    """Generate a PDF with incremental updates (like issue3115)"""
    # Base document
    base = b"""%PDF-1.4
1 0 obj
<< /Type /Catalog /Pages 2 0 R >>
endobj
2 0 obj
<< /Type /Pages /Kids [3 0 R] /Count 1 >>
endobj
3 0 obj
<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R >>
endobj
4 0 obj
<< /Length 20 >>
stream
BT
(Original) Tj
ET
endstream
endobj
xref
0 5
0000000000 65535 f
0000000009 00000 n
0000000058 00000 n
0000000111 00000 n
0000000209 00000 n
trailer
<< /Size 5 /Root 1 0 R >>
startxref
309
%%EOF"""

    # Incremental update - modify object 4
    update = b"""
1 0 obj
<< /Type /Catalog /Pages 2 0 R /Version 1.1 >>
endobj
4 0 obj
<< /Length 20 >>
stream
BT
(Updated) Tj
ET
endstream
endobj
xref
0 5
0000000000 65535 f
0000000009 00000 n
0000000058 00000 n
0000000111 00000 n
0000000369 00000 n
trailer
<< /Size 5 /Root 1 0 R /Prev 309 >>
startxref
469
%%EOF"""

    return base + update

def main():
    fixtures_dir = "/home/gp/Projects/pdf-x/tests/fixtures/pdfs"

    # Generate all test PDFs
    test_pdfs = {
        "xref-stream.pdf": write_xref_stream_pdf(),
        "linearized.pdf": write_linearized_pdf(),
        "compressed-object-stream.pdf": write_compressed_object_stream_pdf(),
        "flatedecode.pdf": write_flatedecode_pdf(),
        "annotation-text.pdf": write_annotation_text_pdf(),
        "bad-xref.pdf": write_bad_xref_pdf(),
        "issue3115.pdf": write_incremental_update_pdf(),
    }

    for filename, content in test_pdfs.items():
        filepath = os.path.join(fixtures_dir, filename)
        with open(filepath, 'wb') as f:
            f.write(content)
        print(f"Generated: {filepath} ({len(content)} bytes)")

if __name__ == "__main__":
    main()
