INSTALL_PREFIX ?= /usr/local/bin

.PHONY: all
all: target/release/backlightd target/release/backlightctl

target/release/backlightd: backlightd/Cargo.toml $(shell ls -1 backlightd/src/*.rs)
	cargo test --frozen --bin backlightd
	cargo build --frozen --release --bin backlightd

target/release/backlightctl: backlightctl/Cargo.toml $(shell ls -1 backlightctl/src/*.rs)
	cargo test --frozen --bin backlightctl
	cargo build --frozen --release --bin backlightctl

.PHONY: install
install:
	install -Dm 755 -t $(INSTALL_PREFIX) target/release/backlightd
	install -Dm 755 -t $(INSTALL_PREFIX) target/release/backlightctl
	install -Dm 755 -t /etc/systemd/system backlightd.service
	sed -i 's+$$INSTALL_PREFIX+$(INSTALL_PREFIX)+g' /etc/systemd/system/backlightd.service
	systemctl daemon-reload
	systemctl enable backlightd
	systemctl restart backlightd

.PHONY: clean
clean:
	rm -rf target/release
