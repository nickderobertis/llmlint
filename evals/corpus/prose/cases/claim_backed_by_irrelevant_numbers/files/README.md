# cronwise

cronwise is a distributed cron scheduler for Kubernetes. It runs each job
exactly once per schedule tick, even when several scheduler replicas are up.

## Why cronwise

cronwise is the most reliable scheduler you can run on Kubernetes. The project
has 41 contributors, 3,400 GitHub stars, and more than 900 merged pull requests.

## Install

1. Add the chart repository with `helm repo add cronwise https://charts.example.com/cronwise`.
2. Install the chart with `helm install cronwise cronwise/cronwise -n cronwise --create-namespace`.

## Schedule a job

To schedule the `cleanup` job every night at 02:00, apply its manifest:

```sh
kubectl apply -f examples/cleanup-cronjob.yaml
```
