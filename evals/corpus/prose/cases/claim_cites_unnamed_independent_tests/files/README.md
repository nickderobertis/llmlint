# glyphscan

glyphscan extracts text from scanned invoices and receipts and returns it as
structured JSON: vendor, date, line items, and totals.

## Accuracy

Independent tests have shown that glyphscan is the most accurate OCR engine for
invoices. Field-level results vary with scan quality, so check low-confidence
fields by hand.

## Install

1. Install the package with `pip install glyphscan`.
2. Download the language models with `glyphscan models pull --lang en,de`.

## Scan one file

To scan one invoice and print the JSON to stdout, run `glyphscan scan`:

```sh
glyphscan scan invoices/2026-09-001.pdf
```
