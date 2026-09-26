# thumbgen

Generate thumbnails for a directory tree of photos, in parallel, skipping
anything that is already up to date.

## Usage

```sh
# Thumbnail a camera dump into ./site/thumbs as 320px WebP files.
thumbgen ~/Pictures/2024 --output-dir ./site/thumbs --size 320

# Short forms work too:
thumbgen raw/ -o out -s 128 -f jpeg
```

The output tree mirrors the input tree, so `raw/a/b.cr2` becomes
`out/a/b.jpg` in the second example.

## Options

| Option | Short | Default | Meaning |
| --- | --- | --- | --- |
| `--size` | `-s` | 256 | Longest edge in pixels. |
| `--format` | `-f` | webp | `jpeg`, `png` or `webp`. |
| `--quality` | `-q` | 80 | Encoder quality, 1-100. PNG ignores it. |
| `--jobs` | `-j` | 0 | Worker threads; 0 uses every CPU. |
| `--force` | | off | Rebuild thumbnails even if they look fresh. |
| `--strip-metadata` | | off | Drop EXIF and XMP from outputs. |

## Orientation

Since 0.4, EXIF orientation is always honoured; there is nothing to configure.

## Performance notes

Decoding RAW files dominates the run time. For large libraries, run on an SSD
and leave `--jobs` at its default.
