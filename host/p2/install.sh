#!/bin/sh
# Executed only INSIDE the disposable Debian build Guest.
set -eu
[ "$(id -u)" -eq 0 ]
systemd-detect-virt --vm >/dev/null
exec > /var/log/elasticpc-build.log 2>&1
set -x
failed() {
    echo ELASTICPC_P2_BUILD_FAILED
    cat /var/log/elasticpc-build.log > /dev/ttyS0 || true
    sync
    systemctl poweroff --no-block
}
trap failed EXIT
mkdir -p /mnt/seed /var/lib/elasticpc /usr/lib/elasticpc /etc/elasticpc
mount -o ro /dev/disk/by-label/cidata /mnt/seed
export DEBIAN_FRONTEND=noninteractive
# No network configured; packages are supplied and authenticated by the builder.
set --
for deb in /mnt/seed/debs/*.deb; do
    name=$(dpkg-deb -f "$deb" Package)
    candidate=$(dpkg-deb -f "$deb" Version)
    installed=$(dpkg-query -W -f='${Version}' "$name" 2>/dev/null || true)
    if [ -n "$installed" ] && dpkg --compare-versions "$installed" ge "$candidate"; then
        continue
    fi
    set -- "$@" "$deb"
done
apt-get --yes --no-install-recommends -o Dir::Etc::sourcelist=/dev/null -o Dir::Etc::sourceparts=- install "$@"
install -m 0755 /mnt/seed/elasticctl /usr/local/bin/elasticctl
install -m 0755 /mnt/seed/host-start /usr/lib/elasticpc/host-start
install -m 0755 /mnt/seed/recovery /usr/lib/elasticpc/recovery
install -m 0644 /mnt/seed/elasticpc-host.service /etc/systemd/system/
install -m 0644 /mnt/seed/elasticpc-recovery.service /etc/systemd/system/
install -m 0644 /mnt/seed/elasticpc-recovery.target /etc/systemd/system/
install -m 0755 /mnt/seed/uninstall /usr/lib/elasticpc/uninstall
install -m 0755 /mnt/seed/owned-efi-entry /usr/lib/elasticpc/owned-efi-entry
systemctl enable elasticpc-host.service
systemctl disable apt-daily.timer apt-daily-upgrade.timer systemd-networkd-wait-online.service || true
# Snapshot original EFI tree; existing Debian fallback remains intact.
(cd /boot/efi && find EFI -type f -exec sha256sum '{}' ';') > /etc/elasticpc/original-efi.sha256
mkdir -p /boot/efi/EFI/ElasticPC
cp -a /boot/efi/EFI/debian/. /boot/efi/EFI/ElasticPC/
(cd /boot/efi && find EFI/ElasticPC -type f -exec sha256sum '{}' ';') > /etc/elasticpc/owned-efi.sha256
root_uuid=$(findmnt -no UUID /)
kernel=/boot/vmlinuz-$(uname -r); initrd=/boot/initrd.img-$(uname -r)
[ -s "$kernel" ] && [ -s "$initrd" ]
boot_uuid=$(grub-probe --target=fs_uuid "$kernel")
kernel=$(grub-mkrelpath "$kernel"); initrd=$(grub-mkrelpath "$initrd")
cat > /etc/grub.d/41_elasticpc_recovery <<EOF
#!/bin/sh
cat <<'ENTRY'
menuentry 'ElasticPC recovery' --id elasticpc-recovery {
    search --no-floppy --fs-uuid --set=root $boot_uuid
    linux $kernel root=UUID=$root_uuid ro console=ttyS0,115200 systemd.unit=elasticpc-recovery.target
    initrd $initrd
}
ENTRY
EOF
chmod 0755 /etc/grub.d/41_elasticpc_recovery
update-grub
# This change is confined to the Guest's private OVMF variable file.
esp=$(findmnt -no SOURCE /boot/efi)
disk=/dev/$(lsblk -no PKNAME "$esp" | head -n1)
part=$(cat "/sys/class/block/$(basename "$esp")/partition")
efibootmgr --create --disk "$disk" --part "$part" --label ElasticPC --loader '\EFI\ElasticPC\shimx64.efi'
/usr/local/bin/elasticctl --version
qemu-system-x86_64 --version | head -n1
virsh --version
(cd /boot/efi && sha256sum -c /etc/elasticpc/original-efi.sha256)
touch /etc/cloud/cloud-init.disabled
printf 'debian-13-p2\n' > /etc/elasticpc/image-version
sync
echo ELASTICPC_P2_BUILD_OK
cat /var/log/elasticpc-build.log > /dev/ttyS0 || true
trap - EXIT
systemctl poweroff --no-block
