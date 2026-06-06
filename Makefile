.PHONY: all build test fmt clean install

PREFIX ?= /usr/local
VERSION ?= 0.1.0

all: build

build:
	cargo build --release
	mkdir -p bin
	cp target/release/ttrack bin/ttrack
	cp target/release/ttrackd bin/ttrackd

test:
	cargo test

fmt:
	cargo fmt --all

install: build
	install -Dm755 bin/ttrack $(DESTDIR)$(PREFIX)/bin/ttrack
	install -Dm755 bin/ttrackd $(DESTDIR)/usr/libexec/ttrackd
	install -Dm644 packaging/ttrackd.service $(DESTDIR)/lib/systemd/system/ttrackd.service
	install -Dm644 packaging/ttrack.conf $(DESTDIR)/etc/ttrack/ttrack.conf
	install -dm700 $(DESTDIR)/var/lib/ttrack
	install -dm750 $(DESTDIR)/var/log/ttrack

clean:
	rm -rf bin target
