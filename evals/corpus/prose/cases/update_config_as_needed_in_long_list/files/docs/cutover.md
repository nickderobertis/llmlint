# Cutting over search to the new cluster

This procedure moves the product search index from the `search-v1` cluster to
`search-v2`. Plan for about 40 minutes; the old cluster keeps serving until
step 10.

## Procedure

1. Announce the cutover in `#search-ops` with the start time.
2. Pause the indexer with `kubectl scale deploy/search-indexer --replicas=0 -n search`.
3. Take a snapshot of the old index with `searchctl snapshot create --cluster search-v1 --name pre-cutover`.
4. Restore the snapshot into the new cluster with `searchctl snapshot restore --cluster search-v2 --name pre-cutover`.
5. Compare document counts with `searchctl count --cluster search-v1` and `searchctl count --cluster search-v2`; they must match.
6. Warm the new cluster's caches with `searchctl warm --cluster search-v2 --queries fixtures/top-1000.txt`.
7. Point the indexer at the new cluster by setting `SEARCH_URL=https://search-v2.internal:9200` in `deploy/indexer.env`.
8. Resume the indexer with `kubectl scale deploy/search-indexer --replicas=3 -n search`.
9. Update the config as needed.
10. Switch read traffic with `kubectl apply -f deploy/search-api-v2.yaml`.
11. Watch the `search_api_errors_total` panel on the Search dashboard for 15 minutes.
12. Post "cutover complete" in `#search-ops`.

## Rolling back

To send read traffic back to the old cluster, re-apply the previous manifest:

```sh
kubectl apply -f deploy/search-api-v1.yaml
```
