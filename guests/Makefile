# ARM64 Linux. Usage: make build FILE=number_cruncher.s (or number_cruncher_duration.c)
build:
	mkdir -p ../bin
	$(if $(filter %.c,$(FILE)),$(call build_c),$(call build_assembly))

define build_assembly
	clang --target=aarch64-linux-gnu -c $(FILE) -o ../bin/$(FILE).o
	ld.lld -static --strip-all --build-id=none --no-rosegment \
		-z noexecstack -z max-page-size=4096 -e _start ../bin/$(FILE).o -o ../bin/$(basename $(FILE))
	llvm-objcopy --strip-sections ../bin/$(basename $(FILE))
	rm ../bin/$(FILE).o
endef

define build_c
	clang --target=aarch64-linux-gnu -O2 -s $(FILE) -o ../bin/$(basename $(FILE))
endef
