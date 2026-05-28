#!/usr/bin/env bash
# Helpers for `mise run test:manual:ramdisk`: ephemeral, per-run RAM-backed
# sandbox under `DAFT_MANUAL_TEST_BASE`.
#
# Design contract (driven by #512 user redirect):
#   - One mount per run, name disambiguated by PID so two concurrent shells
#     don't collide.
#   - Trap-cleared on EXIT/INT/TERM; no persistent volume to manage.
#   - macOS allocates an APFS volume via hdiutil/diskutil; Linux just makes a
#     subdir under the kernel's existing `/dev/shm` tmpfs (no allocation
#     needed).
#   - `cargo test`'s unit-level tests for cow_copy / store / etc. use
#     tempfile::tempdir() and respect $TMPDIR, NOT DAFT_MANUAL_TEST_BASE — so
#     the physical-FS coverage path is left intact by construction.

# Stable diagnostic name; appended with `-$$` so concurrent shells don't fight
# over the same mount.
: "${DAFT_RAMDISK_NAME_PREFIX:=daft-ramdisk-test}"

# macOS only. tmpfs sizing on Linux is owned by the kernel.
: "${DAFT_RAMDISK_SIZE_MB:=4096}"

# Picks the per-run mount name. Pure function so callers can use it both for
# alloc and free without re-derivation drift.
_ramdisk_name() {
  echo "${DAFT_RAMDISK_NAME_PREFIX}-$$"
}

# Allocates the RAM-backed mount and prints the absolute path to stdout.
# Errors are reported on stderr and bubble up via `set -e` in the caller.
ramdisk_alloc() {
  local name
  name=$(_ramdisk_name)

  case "$OSTYPE" in
    darwin*)
      # Validate up-front so a non-numeric `DAFT_RAMDISK_SIZE_MB` produces a
      # readable error instead of bash's `syntax error in expression` abort
      # from the arithmetic expansion below.
      if ! [[ "$DAFT_RAMDISK_SIZE_MB" =~ ^[0-9]+$ ]]; then
        echo "ramdisk: DAFT_RAMDISK_SIZE_MB must be a positive integer (got '$DAFT_RAMDISK_SIZE_MB')" >&2
        return 1
      fi
      # Sectors are 512 bytes each. `hdiutil attach -nomount ram://N` returns
      # a device path on stdout followed by trailing whitespace.
      local sectors=$((DAFT_RAMDISK_SIZE_MB * 1024 * 1024 / 512))
      local dev
      dev=$(hdiutil attach -nomount "ram://$sectors" | awk '{print $1}')
      if [ -z "$dev" ]; then
        echo "ramdisk: hdiutil attach failed" >&2
        return 1
      fi
      # diskutil writes a "Started erase" / "Finished erase" banner to stdout;
      # silence it so the caller can capture the mount path cleanly.
      if ! diskutil erasevolume APFS "$name" "$dev" >/dev/null; then
        hdiutil detach "$dev" >/dev/null 2>&1 || true
        echo "ramdisk: diskutil erasevolume failed" >&2
        return 1
      fi
      echo "/Volumes/$name"
      ;;
    linux*)
      if [ ! -d /dev/shm ]; then
        echo "ramdisk: /dev/shm not available — check your container runtime / mount config" >&2
        return 1
      fi
      local path="/dev/shm/$name"
      mkdir -p "$path"
      echo "$path"
      ;;
    *)
      echo "ramdisk: unsupported platform: $OSTYPE" >&2
      return 1
      ;;
  esac
}

# Frees the mount at $1. Best-effort and silent on no-op so SIGKILL leaks at
# most one stray volume per crash (not silently a fleet of them).
ramdisk_free() {
  local mount=$1
  [ -n "$mount" ] || return 0

  case "$OSTYPE" in
    darwin*)
      if [ -d "$mount" ]; then
        # `diskutil eject` is the documented free; falls back to detach if the
        # volume reports busy (eject sometimes does on a still-open shell).
        if ! diskutil eject "$mount" >/dev/null 2>&1; then
          # Resolve mount → device for the fallback detach. `df` on macOS
          # prints the device as the first field for the matching row.
          local dev
          dev=$(df "$mount" 2>/dev/null | awk 'NR==2 {print $1}')
          if [ -n "$dev" ]; then
            hdiutil detach "$dev" >/dev/null 2>&1 || true
          fi
        fi
      fi
      ;;
    linux*)
      [ -d "$mount" ] && rm -rf "$mount"
      ;;
  esac
}
