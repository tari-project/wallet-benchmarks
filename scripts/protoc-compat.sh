#!/usr/bin/env bash
set -euo pipefail

exec protoc --experimental_allow_proto3_optional "$@"
