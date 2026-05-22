TARGET = aarch64-unknown-linux-musl
BINARY = target/$(TARGET)/release/ais-forwarder
PI = root@merrimac-pi
PI_BIN = /usr/local/bin/ais-forwarder
PI_CFG = /etc/ais-forwarder
SERVICE = merrimac-ais

.PHONY: build install mpi run debug trace

build:
	cargo build --release --target $(TARGET)

install:
	cargo build --release
	cargo test

mpi: build
	ssh $(PI) systemctl stop $(SERVICE) || :
	ssh $(PI) killall -9 ais-forwarder || :
	scp $(BINARY) $(PI):$(PI_BIN)
	ssh $(PI) systemctl start $(SERVICE)

run: mpi
	@# Run with debug logging, output to terminal
	ssh $(PI) systemctl stop $(SERVICE) || :
	(ssh $(PI) ais-forwarder -v 2>&1) | tee /tmp/ais.log

debug: build
	@# Build + run with debug logging without install
	ssh $(PI) systemctl stop $(SERVICE) || :
	ssh $(PI) killall -9 ais-forwarder || :
	scp $(BINARY) $(PI):$(PI_BIN)
	(ssh $(PI) ais-forwarder -v 2>&1) | tee /tmp/ais.log

trace: build
	@# Build + run with trace logging
	ssh $(PI) systemctl stop $(SERVICE) || :
	ssh $(PI) killall -9 ais-forwarder || :
	scp $(BINARY) $(PI):$(PI_BIN)
	(ssh $(PI) ais-forwarder -v -v 2>&1) | tee /tmp/ais.log
