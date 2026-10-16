#!/bin/sh
# Runs only in the isolated build Guest against its file-backed root.
set -eu
systemd-detect-virt --vm >/dev/null
exec > /var/log/elasticpc-bundle-build.log 2>&1
fail() { echo ELASTICPC_BUNDLE_FAILED > /dev/ttyS0; tail -80 /var/log/elasticpc-bundle-build.log > /dev/ttyS0; sync; systemctl poweroff --no-block; }
trap fail EXIT
mkdir -p /mnt/epc-bundle
mount -o ro /dev/disk/by-label/EPCSETUP /mnt/epc-bundle
mkdir -p /var/tmp/epc-payload
tar -xf /mnt/epc-bundle/payload.tar -C /var/tmp/epc-payload
cd /var/tmp/epc-payload
sha256sum -c SHA256SUMS
set --
for deb in ./debs/*.deb; do
 name=$(dpkg-deb -f "$deb" Package);candidate=$(dpkg-deb -f "$deb" Version);installed=$(dpkg-query -W -f='${Version}' "$name" 2>/dev/null || true)
 if [ -n "$installed" ] && dpkg --compare-versions "$installed" ge "$candidate"; then continue; fi
 set -- "$@" "$deb"
done
DEBIAN_FRONTEND=noninteractive apt-get --yes --no-install-recommends -o Dir::Etc::sourcelist=/dev/null -o Dir::Etc::sourceparts=- install "$@"
install -m 0755 epc /usr/local/bin/epc
install -m 0755 collect-diagnostics /usr/local/bin/epc-diagnostics
install -m 0755 check-network /usr/local/bin/epc-network-check
install -m 0755 epc-test /usr/local/bin/epc-test
install -m 0755 epc-input /usr/local/bin/epc-input
install -m 0755 epc-device /usr/local/bin/epc-device
install -m 0755 epc-usb-plan /usr/local/bin/epc-usb-plan
install -m 0755 tailscale /usr/local/bin/tailscale
install -m 0755 tailscaled /usr/local/sbin/tailscaled
install -m 0644 tailscaled.service /etc/systemd/system/tailscaled.service
install -m 0644 profile.json /etc/elasticpc/runtime.json
mkdir -p /usr/share/elasticpc/manjaro /var/lib/elasticpc/driver-packages
chmod 0700 /var/lib/elasticpc/driver-packages
install -m 0644 OVMF_CODE.fd OVMF_VARS.fd /usr/share/elasticpc/
install -m 0644 manjaro-vmlinuz /usr/share/elasticpc/manjaro/vmlinuz
install -m 0644 manjaro-initrd /usr/share/elasticpc/manjaro/initrd
install -m 0644 driver-packages.txt /var/lib/elasticpc/driver-packages/README.txt
install -m 0644 next-boot.txt /root/NEXT-BOOT.txt
chpasswd -e < password-hash
# A private removable EFI bridge chainloads the existing Windows ESP; never edits it.
truncate -s 32M /usr/share/elasticpc/windows-bridge.img
mkfs.vfat -n EPCWINBOOT /usr/share/elasticpc/windows-bridge.img
mmd -i /usr/share/elasticpc/windows-bridge.img ::/EFI ::/EFI/BOOT
mcopy -i /usr/share/elasticpc/windows-bridge.img windows-bridge.efi ::/EFI/BOOT/BOOTX64.EFI
# These settings exist only in the independent USB Host.
printf 'options vfio-pci ids=10de:2c05,10de:22e9\nsoftdep nouveau pre: vfio-pci\n' > /etc/modprobe.d/elasticpc-vfio.conf
printf '\nvfio\nvfio_iommu_type1\nvfio_pci\n' >> /etc/initramfs-tools/modules
printf '\nGRUB_CMDLINE_LINUX="console=ttyS0,115200 console=tty0 rootwait amd_iommu=on iommu=pt"\nGRUB_DISABLE_LINUX_UUID=false\nGRUB_DISABLE_LINUX_PARTUUID=true\n' >> /etc/default/grub.d/99-elasticpc-usb.cfg
sed -i 's/rw,errors=remount-ro/rw,noatime,errors=remount-ro/' /etc/fstab
mkdir -p /etc/systemd/journald.conf.d
printf '[Journal]\nStorage=volatile\nRuntimeMaxUse=32M\n' > /etc/systemd/journald.conf.d/elasticpc-usb.conf
systemctl disable apt-daily.timer apt-daily-upgrade.timer || true
systemctl mask NetworkManager-wait-online.service
systemctl enable tailscaled.service
# No original OS auto-start, no copied Tailscale identity, no enabled SSH server.
touch /etc/elasticpc/runtime-enabled
update-initramfs -u -k all
update-grub
python3 -m py_compile /usr/local/bin/epc /usr/local/bin/epc-diagnostics /usr/local/bin/epc-network-check /usr/local/bin/epc-test
/usr/local/bin/tailscale version
/usr/local/sbin/tailscaled --version
/usr/local/bin/epc --help
websockify --help >/dev/null
systemctl disable elasticpc-bundle-install.service
cd /
rm -rf /var/tmp/epc-payload
sync
trap - EXIT
echo ELASTICPC_BUNDLE_OK > /dev/ttyS0
systemctl poweroff --no-block
