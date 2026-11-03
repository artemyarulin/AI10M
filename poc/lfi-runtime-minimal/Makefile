# Run inside Linux ARM64 with 4 KiB pages. All generated files stay in build/.
# Ubuntu build tools: make clang lld llvm binutils curl, plus Rust via rustup.
COUNT ?= 10
RUSTC = $(HOME)/.cargo/bin/rustc
CARGO = $(HOME)/.cargo/bin/cargo
TOOLS = build/aarch64-lfi-tools/bin

.DELETE_ON_ERROR:
.PHONY: all run clean
all: build/runtime

build:
	mkdir -p build

build/aarch64-lfi-tools/bin/lfi-rewrite: | build
	curl -L https://github.com/lfi-project/lfi/releases/download/v0.10/aarch64-lfi-tools.tar.gz \
		-o build/lfi-tools.tar.gz
	tar -xzf build/lfi-tools.tar.gz -C build

build/guest: main.rs Makefile rust-toolchain.toml $(TOOLS)/lfi-rewrite
	$(RUSTC) --edition=2021 --target aarch64-unknown-linux-musl \
		-O -C panic=abort --crate-type=lib --emit=asm --cfg guest \
		main.rs -o build/guest.native.s
	$(TOOLS)/lfi-rewrite -a arm64 build/guest.native.s -o build/guest.lfi.s
	clang -c build/guest.lfi.s -o build/guest.o
	ld.lld -pie --no-dynamic-linker -e _start --strip-all \
		build/guest.o -o build/guest
	objcopy -O binary --only-section=.text build/guest build/guest.text
	# The specialized loader expects 80 instruction bytes at offset 0x210 (528).
	test "$$(wc -c < build/guest.text)" -eq 80
	cmp -n 80 -i 528:0 build/guest build/guest.text
	$(TOOLS)/lfi-verify build/guest

build/guest.text: build/guest
	objcopy -O binary --only-section=.text build/guest build/guest.text

build/runtime: main.rs Cargo.toml Makefile link.ld rust-toolchain.toml build/guest.text
	CARGO_TARGET_DIR=build/cargo $(CARGO) rustc --release \
		--target aarch64-unknown-linux-musl -- \
		-C relocation-model=static -C link-self-contained=no -C linker=ld.lld \
		-C link-arg=-Tlink.ld -C link-arg=-z -C link-arg=max-page-size=4096 \
		-C link-arg=--gc-sections
	cp build/cargo/aarch64-unknown-linux-musl/release/runtime build/runtime.debug
	# Keep loaded segments; discard symbols, section headers and nonloaded data.
	llvm-objcopy --strip-all --strip-sections build/runtime.debug build/runtime

run: all
	./build/runtime build/guest $(COUNT)

clean:
	rm -rf build
