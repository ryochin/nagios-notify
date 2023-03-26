#!/bin/sh

export RUST_LOG=debug

cargo run -- \
  -v \
  -a "ryo@aquahill.net" \
  -t host \
  -n "RECOVERY" \
  -s "" \
  -H "example.com" \
  -A "192.168.0.1" \
  -S "OK" \
  -d "Wed Sep 20 10:43:55 JST 2023" \
  -o ""
