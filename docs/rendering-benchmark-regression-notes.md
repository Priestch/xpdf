# Rendering Benchmark Regression Notes

This file records the current benchmark comparison used to review future
rendering-pipeline changes. It is intended as a guardrail for other agents:
do not treat a change as an overall improvement just because FAIL/XFAIL
classification changes.

## Benchmark Context

- Corpus: `hayro/hayro-tests/pdfs/custom`
- Harness: `scripts/visual_diff.py`
- Reference renderer: `mutool` at 72 DPI
- Current output captured in: `/tmp/pdfx_visual_diff_recheck_20260710_d.txt`
- Previous same-day output captured in: `/tmp/pdfx_visual_diff_recheck_20260710_c.txt`
- Previous recheck output captured in: `/tmp/pdfx_visual_diff_recheck.txt`
- Previous comparison output captured in: `/tmp/pdfx_visual_diff_latest.txt`
- Recheck date: 2026-07-10
- Harness mode reported by script: `pdf-x native fonts: off`

## Headline Result

Previous comparable result:

```text
PASS: 47  FAIL: 140  XFAIL: 71  XPASS: 0  SKIP: 7  TOTAL: 265
```

Current local result:

```text
PASS: 47  FAIL: 113  XFAIL: 105  XPASS: 0  SKIP: 0  TOTAL: 265
```

Assessment:

- PASS count remains 47.
- FAIL count decreased by 27 against `/tmp/pdfx_visual_diff_latest.txt`, and
  decreased by 1 against the previous same-day recheck. SKIP is now 0.
- Some of the headline-count improvement is classification movement, but there
  are also substantial real pixel-match gains.
- The strongest confirmed rendering improvements are in text clipping, glyph
  clipping, Type1/CID/Type3 font handling, shading-on-text, and a few
  image/mask and real-world PDFs.
- Relative to `/tmp/pdfx_visual_diff_latest.txt`, there are no large confirmed
  rendering regressions. Relative to the previous same-day recheck, there are
  no pixel-match regressions above 0.05%; the material changes improved or were
  classification cleanups, especially password-encrypted files moving from
  SKIP to XFAIL.

Treat the current state as a real partial improvement, but not a complete
milestone: PASS count is not higher than the old baseline, and many improved
or newly unskipped files remain below the 99.5% pass threshold.

## Meaningful Improvements

These are real pixel-match improvements and should be protected in future
changes:

```text
text_rendering_clipping.pdf           1.4%  -> 98.3%
text_rendering_stroking_clipping.pdf  1.4%  -> 97.2%
TextClippingModeChanges.pdf           0.6%  -> 95.9%
OverlappingGlyphClipping.pdf          74.6% -> 93.3%
text-fading.pdf                       71.1% -> 81.4%
font_cid_3.pdf                        80.0% -> 90.2%
font_type1_9.pdf                      83.6% -> 89.6%
font_type1_10.pdf                     81.5% -> 86.7%
arabiccidtruetype-pdf.pdf             95.0% -> 99.2%
issue_scaled_glyph.pdf                85.8% -> 89.9%
image_mask_hairline.pdf               80.4% -> 84.4%
font_cid_1.pdf                        77.1% -> 81.1%
shading_operator_1.pdf                92.0% -> 95.7%
font_type1_8.pdf                      92.1% -> 95.8%
animated-distributions.pdf            90.5% -> 93.9%
font_type3_widths_with_matrix.pdf     59.6% -> 66.7%
pattern_shading_on_text.pdf           85.3% -> 91.4%
fonts_type1_latex.pdf                 93.0% -> 95.9%
font_type1_11.pdf                     91.8% -> 94.7%
image_inline_4.pdf                    88.4% -> 92.5%
font_cid_6.pdf                        93.5% -> 96.2%
pdftc_900k_0930_reduced.pdf           92.9% -> 95.5%
pdftc_100k_1894.pdf                   54.4% -> 56.8%
pdftc_900k_0974_page_1.pdf            83.2% -> 95.1%
font_truetype_1.pdf                   92.6% -> 94.9%
pdftc_900k_0319_page_1.pdf            80.9% -> 83.7%
issue53.pdf                           0.0%  -> 86.0%
password_1.pdf                        0.0%  -> 97.9%
password_2.pdf                        0.0%  -> 98.0%
password_encrypted_aes_128.pdf        0.0%  -> 97.9%
password_encrypted_aes_256.pdf        0.0%  -> 97.9%
password_encrypted_rc4_128.pdf        0.0%  -> 97.9%
password_encrypted_rc4_40.pdf         0.0%  -> 97.9%
image_inline_7.pdf                    94.2% -> 96.3%
tile_clamp_bug.pdf                    42.0% -> 47.8%
font_cid_5.pdf                        95.0% -> 96.9%
issue_panic_with_clip.pdf             96.6% -> 98.2%
font_vertical.pdf                     83.5% -> 85.1%
font_cid_4.pdf                        96.0% -> 97.5%
font_cid_7.pdf                        89.6% -> 91.0%
```

The largest confirmed win is the text clipping path:

- `text_rendering_clipping.pdf`
- `text_rendering_stroking_clipping.pdf`
- `TextClippingModeChanges.pdf`

These jumped from near-total mismatch to roughly 96-98% pixel match. The
2026-07-10 rerun also shows `OverlappingGlyphClipping.pdf` reaching 93.3%,
which is a large improvement over the earlier 88.4% interim result.

Additional 2026-07-10 improvements over the 2026-07-09 recheck:

```text
pattern_shading_on_text.pdf           85.3% -> 91.4%
font_type3_widths_with_matrix.pdf     62.8% -> 66.7%
image_inline_3.pdf                    48.5% -> 51.8%
pdftc_900k_0974_page_1.pdf            83.2% -> 85.3%
text_stroked_complex_paint.pdf        74.4% -> 76.3%
image_inline_4.pdf                    91.2% -> 92.5%
image_jbig2_crash.pdf                 96.7% -> 97.8%
font_type3_stroked_glyphs.pdf         78.5% -> 79.5%
tile_clamp_bug.pdf                    41.3% -> 42.2%
```

Additional same-day 2026-07-10 improvements over the previous 2026-07-10
recheck:

```text
password_2.pdf                        0.0% -> 98.0%  SKIP -> XFAIL
password_1.pdf                        0.0% -> 97.9%  SKIP -> XFAIL
password_encrypted_aes_128.pdf        0.0% -> 97.9%  SKIP -> XFAIL
password_encrypted_aes_256.pdf        0.0% -> 97.9%  SKIP -> XFAIL
password_encrypted_rc4_128.pdf        0.0% -> 97.9%  SKIP -> XFAIL
password_encrypted_rc4_40.pdf         0.0% -> 97.9%  SKIP -> XFAIL
pdftc_900k_0907.pdf                  91.9% -> 93.5%
issue_stream_abbreviations.pdf       96.3% -> 96.4%
```

## Regressions To Investigate

Only these files show negative deltas against `/tmp/pdfx_visual_diff_latest.txt`;
they should be checked before accepting future rendering changes as globally
better:

```text
font_truetype_2.pdf                96.7% -> 96.6%
font_truetype_4.pdf                99.7% -> 99.6%
stream_ccit_not_enough_data.pdf    87.2% -> 87.1%
```

Current regression assessment:

- No large current rendering regressions. The remaining negative pixel deltas
  are noise-level changes relative to `/tmp/pdfx_visual_diff_latest.txt`.
- `stream_ccit_not_enough_data.pdf` remains a malformed/truncated CCITT
  sentinel, but the 2026-07-10 run improved slightly over the 2026-07-09
  recheck.
- `tile_clamp_bug.pdf` is still materially broken, but it improved again in
  the latest same-day run and is no longer a regression against the older
  baseline.

Resolved encrypted/object-stream SKIP fluctuation from the previous same-day
run:

```text
catalog-in-objstm-aes.pdf                SKIP -> XFAIL   0.0% -> 98.9%
encrypted_aes_256.pdf                    SKIP -> XFAIL   0.0% -> 97.1%
encrypted_aes_256_hardened.pdf           SKIP -> XFAIL   0.0% -> 97.1%
metadata_in_object_stream_encrypted.pdf  SKIP -> PASS    0.0% -> 100.0%
```

The previous run's encrypted-file SKIPs should be treated as a transient
classification/harness issue unless reproduced again.

Resolved password-encrypted SKIPs in the latest run:

```text
password_1.pdf                         SKIP -> XFAIL  0.0% -> 97.9%
password_2.pdf                         SKIP -> XFAIL  0.0% -> 98.0%
password_encrypted_aes_128.pdf         SKIP -> XFAIL  0.0% -> 97.9%
password_encrypted_aes_256.pdf         SKIP -> XFAIL  0.0% -> 97.9%
password_encrypted_rc4_128.pdf         SKIP -> XFAIL  0.0% -> 97.9%
password_encrypted_rc4_40.pdf          SKIP -> XFAIL  0.0% -> 97.9%
```

The current benchmark has no SKIP entries.

Previously suspected regressions such as `font_vertical.pdf`,
`image_mask_missing_bpc.pdf`, `font_truetype_7.pdf`, `font_truetype_6.pdf`,
`stream_jpx_3.pdf`, and several `pdftc` files are not regressions in the latest
recheck against `/tmp/pdfx_visual_diff_latest.txt`; they are stable or improved.

## Status Classification Changes

The following changed status between the previous comparable result and the
current run:

```text
arabiccidtruetype-pdf.pdf                FAIL  -> XFAIL  95.0% -> 99.2%
catalog-in-objstm-aes.pdf                FAIL  -> XFAIL  98.8% -> 98.9%
encrypted_aes_128.pdf                    FAIL  -> XFAIL  96.8% -> 97.1%
encrypted_aes_256.pdf                    FAIL  -> XFAIL  96.8% -> 97.1%
encrypted_aes_256_hardened.pdf           FAIL  -> XFAIL  96.8% -> 97.1%
encrypted_rc4_rev2.pdf                   FAIL  -> XFAIL  96.8% -> 97.1%
encrypted_rc4_rev3.pdf                   FAIL  -> XFAIL  96.8% -> 97.1%
font_type1_1.pdf                         FAIL  -> XFAIL  98.3% -> 99.3%
font_type1_5.pdf                         FAIL  -> XFAIL  98.5% -> 99.1%
image_interpolate.pdf                    FAIL  -> XFAIL  98.0% -> 98.0%
image_jbig2_crash.pdf                    FAIL  -> XFAIL  96.7% -> 97.8%
image_mask_missing_bpc.pdf               XFAIL -> FAIL   90.3% -> 90.5%
issue11.pdf                              FAIL  -> XFAIL  99.4% -> 99.4%
issue53.pdf                              SKIP  -> XFAIL   0.0% -> 86.0%
password_1.pdf                           SKIP  -> XFAIL   0.0% -> 97.9%
password_2.pdf                           SKIP  -> XFAIL   0.0% -> 98.0%
password_encrypted_aes_128.pdf           SKIP  -> XFAIL   0.0% -> 97.9%
password_encrypted_aes_256.pdf           SKIP  -> XFAIL   0.0% -> 97.9%
password_encrypted_rc4_128.pdf           SKIP  -> XFAIL   0.0% -> 97.9%
password_encrypted_rc4_40.pdf            SKIP  -> XFAIL   0.0% -> 97.9%
path_rendering_1.pdf                     FAIL  -> XFAIL  98.4% -> 98.4%
path_rendering_9.pdf                     FAIL  -> XFAIL  98.3% -> 98.3%
pattern_shading_on_text.pdf              FAIL  -> XFAIL  85.3% -> 91.4%
pattern_shading_type1_3.pdf              FAIL  -> XFAIL  98.9% -> 98.9%
pdftc_900k_0917_page_1_reduced.pdf       FAIL  -> XFAIL  99.1% -> 99.3%
pdftc_900k_0974_page_1.pdf               FAIL  -> XFAIL  83.2% -> 95.1%
text_stroked_complex_paint.pdf           FAIL  -> XFAIL  74.4% -> 76.3%
stream_jpx_6.pdf                         FAIL  -> XFAIL  63.2% -> 63.2%
```

Do not count a FAIL-to-XFAIL move as a rendering improvement unless the pixel
percentage also improves.

## Acceptance Guidance For Future Agent Changes

When evaluating another agent's rendering changes:

1. Re-run the full benchmark:

   ```bash
   python3 scripts/visual_diff.py > /tmp/pdfx_visual_diff_current.txt 2>&1
   ```

2. Compare against the previous recorded output, not just the summary counts.

3. Require that any claimed improvement preserves the large text clipping wins:

   ```text
   text_rendering_clipping.pdf           >= 98.3%
   text_rendering_stroking_clipping.pdf  >= 97.2%
   TextClippingModeChanges.pdf           >= 95.9%
   OverlappingGlyphClipping.pdf          >= 93.3%
   pattern_shading_on_text.pdf           >= 91.4%
   font_type3_widths_with_matrix.pdf     >= 66.7%
   tile_clamp_bug.pdf                    >= 47.8%
   ```

4. Treat these as regression sentinels:

   ```text
   tile_clamp_bug.pdf
   stream_ccit_not_enough_data.pdf
   metadata_in_object_stream_encrypted.pdf
   catalog-in-objstm-aes.pdf
   encrypted_aes_256.pdf
   encrypted_aes_256_hardened.pdf
   password_1.pdf
   password_2.pdf
   password_encrypted_aes_128.pdf
   password_encrypted_aes_256.pdf
   password_encrypted_rc4_128.pdf
   password_encrypted_rc4_40.pdf
   font_type3_widths_with_matrix.pdf
   pattern_shading_on_text.pdf
   font_vertical.pdf
   image_mask_missing_bpc.pdf
   font_truetype_7.pdf
   font_truetype_6.pdf
   pdftc_100k_1894.pdf
   ```

5. Consider a change cleanly positive only if:

   - PASS count increases, or important low-match PDFs improve materially.
   - No sentinel file regresses meaningfully.
   - XFAIL movement is explained separately from pixel-match movement.

## Current Engineering Interpretation

The current local changes improved text clipping / glyph clipping / CID,
Type1, and Type3 font rendering behavior, plus some text-painted-with-pattern
and image edge cases. The latest same-day rerun also eliminated all SKIPs by
classifying password-encrypted cases as XFAIL.
However, it has not yet moved final acceptance, because PASS is still 47 and
many improved files remain below the 99.5% pass threshold.

Recommended next investigation order:

1. Preserve and understand the text clipping improvements.
2. Preserve the 2026-07-10 Type3 and shading-on-text improvements.
3. Investigate why high-90% font/text cases still miss the 99.5% threshold.
4. Continue broad font/text work only with per-file before/after tracking.
