# Sourced inside the isolated build Guest, after the P2 installation.
set -eu
printf 'elasticpc-host\n' > /etc/hostname
systemctl mask ssh.service ssh.socket
systemctl disable systemd-networkd.service systemd-networkd-wait-online.service || true
systemctl enable NetworkManager.service
rm -f /etc/systemd/network/*cloud-init* /etc/network/interfaces.d/*cloud-init*
mkdir -p /etc/NetworkManager/conf.d
printf '[main]\nno-auto-default=*\n' > /etc/NetworkManager/conf.d/10-no-automatic-connections.conf
chpasswd -e < /mnt/seed/usb-root-password-hash
printf 'GRUB_TIMEOUT=5\nGRUB_TERMINAL="console serial"\nGRUB_SERIAL_COMMAND="serial --speed=115200"\nGRUB_CMDLINE_LINUX="console=ttyS0,115200 console=tty0 rootwait"\nGRUB_DISABLE_OS_PROBER=true\n' > /etc/default/grub.d/99-elasticpc-usb.cfg
for module in usb_storage uas xhci_pci xhci_hcd sd_mod ext4; do
 printf '%s\n' "$module" >> /etc/initramfs-tools/modules
done
# Never auto-grow onto the fake advertised capacity, or issue discard requests.
touch /etc/growroot-disabled
if dpkg-query -W -f='${Status}' cloud-initramfs-growroot 2>/dev/null | grep -q 'install ok installed'; then
 dpkg --purge cloud-initramfs-growroot
fi
sed -i 's/,discard//g; s/discard,//g' /etc/fstab
systemctl mask fstrim.service fstrim.timer systemd-growfs-root.service systemd-repart.service
update-initramfs -u -k all
update-grub
mkdir -p /boot/efi/EFI/BOOT
cp /boot/efi/EFI/debian/shimx64.efi /boot/efi/EFI/BOOT/BOOTX64.EFI
cp /boot/efi/EFI/debian/grubx64.efi /boot/efi/EFI/BOOT/grubx64.efi
cp /boot/efi/EFI/debian/grub.cfg /boot/efi/EFI/BOOT/grub.cfg
if [ -f /boot/efi/EFI/debian/mmx64.efi ]; then cp /boot/efi/EFI/debian/mmx64.efi /boot/efi/EFI/BOOT/mmx64.efi; fi
# Keep the backup GPT within the 12 GiB image, never at a fake USB's reported end.
# cloud-init has already grown the file-backed build disk; it is disabled for physical boot.
cat > /etc/motd <<'MSG'
ElasticPC USB Host - physical validation build
No existing OS disk is mounted or imported automatically. SSH is disabled.
Run: elasticctl doctor --json
Network setup: nmtui (no saved credentials copied from your original OS)
Shutdown: poweroff
MSG
# Make readiness output usable on both a local screen and the VM serial console.
sed -i 's/echo ELASTICPC_HOST_READY > \/dev\/ttyS0/echo ELASTICPC_HOST_READY/' /usr/lib/elasticpc/host-start
install -m 0755 /mnt/seed/usb-verify /usr/lib/elasticpc/usb-verify
install -m 0644 /mnt/seed/usb-verify.service /etc/systemd/system/
systemctl enable usb-verify.service
systemctl enable getty@tty1.service serial-getty@ttyS0.service
# No root password or identity inherited from the original host.
