.PHONY: install-chrome-host

install-chrome-host:
	cargo build --release --locked
	./target/release/vpn-ru-proxy --install-chrome-host "$(EXTENSION_ID)"
