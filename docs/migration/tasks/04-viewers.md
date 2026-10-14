# MIME, hex and image tasks

## V01 — MIME/language detection and routing

**Status:** todo

**Depends:** F02, F03. **Owns:** `kode-workspace/src/mime/`,
`resources/mime-table.json` and routing DTOs.
**Read:** `editor/mime/*.kt`, `DefaultMimeTypeDetectorTest`, source `samples/`.

1. Port explicit filename/extension/language/category mappings, including systemd
   units, C+ `.cp`, C/C++ header variants and extension-less scripts.
2. Use `infer` signatures and `mime_guess` as helpers. Preserve precedence: shell
   shebang before known filename, explicit filename before ordinary signatures,
   then bounded content/text fallback. Audit platform probe differences.
3. Return classification source, MIME, language and viewer kind. Route text to
   editor, supported image to viewer, binary/unknown to hex; decoding failure is
   visible and can offer hex inspection.

**Accept:** complete sample MIME matrix and original detector tests; missing or
   misleading extensions, unknown text/binary, shebang variants, overrides and
   bounded reads. Do not treat every image MIME as a proven decoder capability.

## V02 — Byte buffer and hex viewer

**Status:** todo

**Depends:** F05, V01, E04. **Owns:** `kode-core/src/document/bytes.rs`,
`kode-tui/src/viewers/hex.rs`.
**Read:** `IByteBuffer.kt`, `ByteBuffer.kt`, `BinaryHexView.kt`, HelpView hex bindings.

1. Port byte navigation, selection, insert/delete, clone, clipboard conversion,
   slices and cursor bounds. Keep byte offsets independent from text coordinates.
2. Render offsets, paired hex/ASCII cells and shared selection/cursor; support
   row/page movement, drag, wheel, Home/End and Alt+C/X/V.
3. Integrate viewer routing/recents and small-terminal layout. Audit actual source
   mutation/save affordances: its view exposes deletion/paste, but does not supply
   a complete direct hex-digit editing/save workflow. Record rather than invent
   such functionality as baseline.

**Accept:** empty/partial rows, byte values 0–255, reverse selection, row boundaries,
   page clamp, paste/delete and both display-column mouse mappings. No corruption
   from using UTF-8 indexes for bytes. Any added save behavior is explicit.

## V03 — Image decoding and colored cell modes

**Status:** todo

**Depends:** F03, F05, V01. **Owns:** `kode-platform/src/images/`,
`kode-tui/src/viewers/image.rs` and image fixtures.
**Read:** `ImageLoader.kt`, `AsciiCore.kt`, all `*AsciiImageRenderer.kt`,
`ImageViewerView.kt`, `KorimAsciiImageRendererTest`.

1. Establish actual decoder coverage using image/resvg/psd and ZIP preview loading:
   PNG, BMP, GIF, TGA, DDS, PSD, KRA and SVG explicitly configured by the source;
   audit samples for other routed formats. Resolve unsupported subformats before
   claiming complete coverage.
2. Port bixel/demi/quart/quadrant/Braille and ASCII helpers as appropriate to their
   active callers, color sampling, aspect compensation and threshold/contrast math.
   Decode/resample in workers and reject outdated resize results.
3. Port mode toggle, width/gray/contrast sliders, keyboard width adjustments and
   drag/release behavior. Preserve portable cell rendering in SSH/ttyd.

**Accept:** one fixture per format and active mode, transparent images, tiny/large
   aspect ratios, invalid data, bounded decode memory and stale job cancellation.
   Golden glyph/color samples plus manual terminal display. A decoder exception
   must not crash the IDE or silently claim success.
