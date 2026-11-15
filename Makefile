INSTALL_PREFIX ?= /usr/local/bin
CONFIG_DIR ?= /etc/backlightd

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
	install -d -m 755 $(CONFIG_DIR)
	test -e $(CONFIG_DIR)/config.toml || install -m 644 etc/backlightd/config.toml $(CONFIG_DIR)/config.toml
	install -Dm 755 -t $(INSTALL_PREFIX) target/release/backlightd
	install -Dm 755 -t $(INSTALL_PREFIX) target/release/backlightctl
	install -Dm 644 -t /etc/systemd/system etc/systemd/system/backlightd.service
	sed -i 's+$$INSTALL_PREFIX+$(INSTALL_PREFIX)+g' /etc/systemd/system/backlightd.service
	install -Dm 644 -t /etc/udev/rules.d etc/udev/rules.d/95-backlightd.rules
	sed -i 's+$$INSTALL_PREFIX+$(INSTALL_PREFIX)+g' /etc/udev/rules.d/95-backlightd.rules
	udevadm control --reload-rules
	systemctl daemon-reload
	systemctl enable backlightd
	systemctl restart backlightd

.PHONY: clean
clean:
	rm -rf target/release
