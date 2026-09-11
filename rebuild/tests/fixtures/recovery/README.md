# Recovery attachment fixture

`queued-draft.pdf` is a deterministic, synthetic one-page PDF 1.4 with a catalog,
page tree, Helvetica text stream, byte-offset cross-reference table and trailer.
It contains no scripts, forms, embedded files or personal data. Generated locally
with Python's standard library. It is deliberately a real PDF, rather than an
arbitrary byte string with a PDF filename.

Independent Poppler checks: `pdfinfo queued-draft.pdf` reports one letter-sized
page, 612 bytes, PDF 1.4; `pdftotext queued-draft.pdf -` returns
`Nuncio offline recovery fixture`. Storage restore tests compare attachment/blob
and frozen MIME rows byte-for-byte. Actual recovery daemon/CLI E2E remains a
separate required check.
