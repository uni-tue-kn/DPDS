BIN := target/release/dpds

# Empty by default: the `telemetry` feature is off unless requested
# (`make build FEATURES=telemetry`), so a plain `make build` produces a binary
# with zero instrumentation overhead.
FEATURES ?=
CARGO_FEATURES := $(if $(FEATURES),--features $(FEATURES))

.PHONY: build check-static-link
.DEFAULT_GOAL := build

# Maximum-performance build: cross-language LTO inlines DPDK's rx_burst/
# tx_burst fast path into the Rust loops. Needs a clang/lld with the same LLVM
# major as rustc (README, "Building"). If this fails with "Unable to find
# libclang" or an lld/bitcode mismatch, that is a host toolchain problem to
# fix, not something to route around here.
build:
	cargo build --release -p dpds $(CARGO_FEATURES)

# Verifies the LTO inlining in `build` actually happened: fails if any call
# instruction in $(BIN) still targets one of the per-packet DPDK functions
# below, and lists each caller -> callee. Without working cross-language LTO
# (clang's LLVM major != rustc's) bindgen's `*__extern` wrappers become real
# calls.
# LTO_DPDK_BIG is listed but never fails the check: the inliner may keep these
# bulk-free wrappers out of line even with LTO working (preof saw that for
# rte_mbuf_raw_free_bulk, whose inlining it never observed). Whether it does
# for DPDS is not yet observed.
# When the hot path starts calling another static-inline DPDK function, add
# its `__extern` wrapper to one of these lists, otherwise a silent LTO failure
# leaves it an out-of-line call nobody notices.
# Matches x86_64 (`call`/`jmp`, space before the operand) and AArch64 (`bl`/
# `b`, tab); `+0x` targets are jumps within one function. Needs objdump.
LTO_DPDK_HOT := (rx_burst|tx_burst|rdtsc)__extern
LTO_DPDK_BIG := (mbuf_raw_free_bulk|pktmbuf_free_bulk)__extern
check-static-link: build
	@echo "rustc $$(rustc -vV | sed -n 's/^LLVM version: /LLVM /p'), $$(clang --version 2>/dev/null | head -1 || echo 'clang: not found')"
	@calls=$$(objdump -d -C --no-show-raw-insn $(BIN) | awk -v pat='$(LTO_DPDK_HOT)|$(LTO_DPDK_BIG)' ' \
		/^[0-9a-f]+ <.*>:$$/ { fn = $$0; sub(/^[0-9a-f]+ </, "", fn); sub(/>:$$/, "", fn); next } \
		/\t(call|callq|bl|jmp|b)[ \t]/ && match($$0, /<[^>]+>/) { \
			callee = substr($$0, RSTART + 1, RLENGTH - 2); \
			if (callee ~ pat && callee !~ /\+0x/) print fn "  ->  " callee \
		}' | sort | uniq -c | sort -rn); \
	big=$$(echo "$$calls" | grep -E '$(LTO_DPDK_BIG)$$'); \
	bad=$$(echo "$$calls" | grep -vE '$(LTO_DPDK_BIG)$$' | grep .); \
	if [ -n "$$big" ]; then \
		echo "Note: out-of-line calls that are not a failure (inliner size decision):"; \
		echo "$$big"; \
	fi; \
	if [ -z "$$bad" ]; then \
		echo "OK: no out-of-line calls to the hot DPDK burst/rdtsc wrappers -- LTO inlining succeeded."; \
	else \
		echo "FAIL: out-of-line calls to hot functions (count, caller -> callee):"; \
		echo "$$bad"; \
		echo "Check that clang and rustc report the same LLVM major version (see README.md)."; \
		exit 1; \
	fi
