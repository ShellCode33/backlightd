PREFIX ?= /usr/local
INSTALL_PREFIX ?= $(PREFIX)/bin
CONFIG_DIR ?= /etc/backlightd
SYSTEMD_UNIT_DIR ?= /etc/systemd/system
UDEV_RULES_DIR ?= /etc/udev/rules.d
DOC_DIR ?= $(PREFIX)/share/doc/backlightd
BUILD_DIR ?= target/release
DESTDIR ?=

.PHONY: all
all:
	cargo build --workspace --release --locked

# Copy files only: safe for package builders and never operates on host services.
.PHONY: install
install:
	install -d -m 755 "$(DESTDIR)$(CONFIG_DIR)"
	test -e "$(DESTDIR)$(CONFIG_DIR)/config.toml" || install -m 644 etc/backlightd/config.toml "$(DESTDIR)$(CONFIG_DIR)/config.toml"
	install -Dm 755 "$(BUILD_DIR)/backlightd" "$(DESTDIR)$(INSTALL_PREFIX)/backlightd"
	install -Dm 755 "$(BUILD_DIR)/backlightctl" "$(DESTDIR)$(INSTALL_PREFIX)/backlightctl"
	install -Dm 644 etc/systemd/system/backlightd.service "$(DESTDIR)$(SYSTEMD_UNIT_DIR)/backlightd.service"
	sed -i 's+$$INSTALL_PREFIX+$(INSTALL_PREFIX)+g' "$(DESTDIR)$(SYSTEMD_UNIT_DIR)/backlightd.service"
	install -Dm 644 etc/udev/rules.d/95-backlightd.rules "$(DESTDIR)$(UDEV_RULES_DIR)/95-backlightd.rules"
	sed -i 's+$$INSTALL_PREFIX+$(INSTALL_PREFIX)+g' "$(DESTDIR)$(UDEV_RULES_DIR)/95-backlightd.rules"
	install -Dm 644 README.md "$(DESTDIR)$(DOC_DIR)/README.md"
	install -Dm 644 LICENSE "$(DESTDIR)$(DOC_DIR)/LICENSE"

# Explicit opt-in for a local source installation, never used by package builders.
.PHONY: activate
activate:
	udevadm control --reload-rules
	systemctl daemon-reload
	systemctl enable --now backlightd

.PHONY: clean
clean:
	cargo clean --release
