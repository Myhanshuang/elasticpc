# Explicit, offline project entry points. No target installs system packages.
CARGO ?= cargo
PYTHON ?= /usr/bin/python3
LAB_RUNTIME ?= /tmp/elasticpc-integration-runtime
LAB_REPORT ?= reports/private-lab-latest.json

.PHONY: check check-native private-lab-build private-lab-dry-run private-lab
check:
	$(CARGO) fmt --all -- --check
	$(CARGO) test --workspace --all-targets --locked --offline
	$(CARGO) clippy --workspace --all-targets --locked --offline -- -D warnings
	$(PYTHON) -m unittest discover -s tests/private_lab
	$(PYTHON) -m unittest discover -s tests/p2
	$(PYTHON) -m unittest discover -s tests/p3
	$(PYTHON) -m unittest discover -s tests/experience
	$(PYTHON) -m unittest discover -s tests/boot_bundle

check-native:
	PKG_CONFIG_PATH=$(LAB_RUNTIME)/root/usr/lib/pkgconfig LD_LIBRARY_PATH=$(LAB_RUNTIME)/root/usr/lib $(CARGO) test --workspace --all-features --all-targets --locked --offline
	PKG_CONFIG_PATH=$(LAB_RUNTIME)/root/usr/lib/pkgconfig LD_LIBRARY_PATH=$(LAB_RUNTIME)/root/usr/lib $(CARGO) clippy --workspace --all-features --all-targets --locked --offline -- -D warnings

private-lab-build:
	$(CARGO) build -p elastic-controller --example private_controller --locked --offline

private-lab-dry-run:
	$(PYTHON) scripts/private_guest_lab.py --runtime $(LAB_RUNTIME) --report $(LAB_REPORT) --dry-run

# Creates KVM Guests and a private daemon: explicit host lab authorization required.
private-lab: private-lab-build
	$(PYTHON) scripts/private_guest_lab.py --runtime $(LAB_RUNTIME) --report $(LAB_REPORT)

# File-backed Debian Host only; dependency preparation uses explicit downloads.
P2_OUTPUT ?= target/p2-host
.PHONY: p2-inputs p2-dry-run p2-host
p2-inputs:
	$(PYTHON) scripts/prepare_p2_inputs.py
	$(PYTHON) scripts/prepare_p2_debs.py
	$(PYTHON) scripts/verify_p2_debian_release.py

p2-dry-run:
	$(PYTHON) scripts/p2_host_image.py --output $(P2_OUTPUT) --dry-run

p2-host:
	$(PYTHON) scripts/p2_host_image.py --output $(P2_OUTPUT)

P3_OUTPUT ?= target/p3-trial
.PHONY: p3-check p3-dry-run p3-lab
p3-check:
	$(PYTHON) -m unittest discover -s tests/p3 -v

p3-dry-run:
	$(PYTHON) scripts/p3_trial_lab.py --output $(P3_OUTPUT) --dry-run

# Explicitly authorized KVM lab, using a private copy of the P2 release.
p3-lab:
	$(PYTHON) scripts/p3_trial_lab.py --output $(P3_OUTPUT)

.PHONY: preview-build preview-check preview-init preview-start preview-console preview-stop
preview-build:
	$(CARGO) build -p elastic-controller --example experience_controller --locked --offline
	$(PYTHON) scripts/package_experience.py

preview-check:
	$(PYTHON) -m unittest discover -s tests/experience -v

preview-init:
	./elasticpc init

preview-start:
	./elasticpc start

preview-console:
	./elasticpc console

preview-stop:
	./elasticpc stop
