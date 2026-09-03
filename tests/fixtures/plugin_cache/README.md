# Plugin-cache golden files

The committed golden for the plugin cache's on-disk metadata — the shape
`llmlint::io::plugins::CacheMeta` writes beside each cached entry, versioned by
`CACHE_SCHEMA`.

Each file is **byte-for-byte what the real writer produces** (`write_meta`, i.e.
`serde_json::to_string_pretty` plus a trailing newline), so it pins the field
names, their order, the schema value, and which fields are omitted — not just
that the types happen to round-trip. `cache_metadata_v1_matches_the_committed_golden`
and its neighbours in `src/io/plugins.rs` assert against these files.

- `v1.json` — an entry carrying both revalidation validators.
- `v1-no-validators.json` — an origin that supplied neither, proving `etag` and
  `last_modified` are omitted rather than written as nulls, and read back as
  `None`.

Changing the shape means bumping `CACHE_SCHEMA` and updating these files in the
same change; a host that already holds entries reads them with the old shape.
