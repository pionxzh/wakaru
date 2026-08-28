#!/bin/bash
# guard-rss.sh — run a command under a resident-memory ceiling.
#
# usage: scripts/guard-rss.sh <limit-gb> [timeout-s] -- <command> [args...]
#
# Kills the command once its RSS exceeds <limit-gb> (integer gigabytes) or it
# runs past [timeout-s] (default 1800). Prints one GUARD summary line with the
# verdict, peak RSS, and elapsed time, and exits with the command's status.
# Pick a ceiling that leaves the machine usable — for example, half of
# physical RAM. Where a stack sampler is available, a sample is captured just
# before a kill so the runaway allocation site is not lost with the process.
set -u

usage() {
  echo "usage: $0 <limit-gb> [timeout-s] -- <command> [args...]" >&2
  exit 2
}

[ $# -ge 3 ] || usage
limit_gb=$1
shift
case "$limit_gb" in
'' | *[!0-9]*) usage ;;
esac
[ "$limit_gb" -gt 0 ] || usage

timeout_s=1800
if [ "$1" != "--" ]; then
  timeout_s=$1
  shift
  case "$timeout_s" in
  '' | *[!0-9]*) usage ;;
  esac
fi
[ "${1:-}" = "--" ] || usage
shift
[ $# -gt 0 ] || usage

limit_kb=$((limit_gb * 1024 * 1024))

"$@" &
pid=$!
peak_kb=0
start=$SECONDS
verdict=ok
sample_file=""

capture_sample() {
  if command -v sample >/dev/null 2>&1; then
    sample_file=$(mktemp -t guard-rss-sample)
    sample "$pid" 2 -file "$sample_file" >/dev/null 2>&1 || sample_file=""
  fi
}

while kill -0 "$pid" 2>/dev/null; do
  rss_kb=$(ps -o rss= -p "$pid" 2>/dev/null | tr -d ' ')
  if [ -n "${rss_kb:-}" ] && [ "$rss_kb" -gt "$peak_kb" ]; then
    peak_kb=$rss_kb
  fi
  if [ -n "${rss_kb:-}" ] && [ "$rss_kb" -gt "$limit_kb" ]; then
    verdict=rss-killed
    capture_sample
    kill -KILL "$pid" 2>/dev/null
    break
  fi
  if [ $((SECONDS - start)) -gt "$timeout_s" ]; then
    verdict=timeout-killed
    capture_sample
    kill -KILL "$pid" 2>/dev/null
    break
  fi
  sleep 2
done

wait "$pid" 2>/dev/null
code=$?
summary="GUARD: verdict=$verdict exit=$code peak_rss_mb=$((peak_kb / 1024)) elapsed_s=$((SECONDS - start))"
if [ -n "$sample_file" ]; then
  summary="$summary sample=$sample_file"
fi
echo "$summary" >&2
exit "$code"
