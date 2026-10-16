/* Fixed x86-64 Linux PID 1: prove userspace entry, then request guest poweroff.
 * No libc, shell, filesystem mounts, network, or host execution is involved.
 * @NONCE@ is replaced only with generated ASCII hexadecimal bytes.
 */
.section .text
.global _start
_start:
    mov $39, %rax                    /* getpid */
    syscall
    cmp $1, %rax
    jne fail
    mov $63, %rax                    /* uname */
    lea utsname(%rip), %rdi
    syscall
    test %rax, %rax
    jne fail
    mov $1, %rax
    mov $1, %rdi
    lea ready(%rip), %rsi
    mov $(ready_end-ready), %rdx
    syscall
    cmp $(ready_end-ready), %rax
    jne fail
    mov $1, %rax
    mov $1, %rdi
    lea poweroff(%rip), %rsi
    mov $(poweroff_end-poweroff), %rdx
    syscall
    cmp $(poweroff_end-poweroff), %rax
    jne fail
    mov $162, %rax                   /* sync: only the guest's ramfs exists */
    syscall
    mov $169, %rax                   /* reboot(LINUX_REBOOT_CMD_POWER_OFF) */
    mov $0xfee1dead, %rdi
    mov $672274793, %rsi
    mov $0x4321fedc, %rdx
    xor %r10, %r10
    syscall
fail:
    mov $1, %rax
    mov $1, %rdi
    lea failed(%rip), %rsi
    mov $(failed_end-failed), %rdx
    syscall
    mov $60, %rax
    mov $1, %rdi
    syscall
.section .rodata
ready: .ascii "ELASTICPC_GUEST_READY:@NONCE@:PID1\n"
ready_end:
poweroff: .ascii "ELASTICPC_GUEST_POWEROFF:@NONCE@\n"
poweroff_end:
failed: .ascii "ELASTICPC_GUEST_FAILURE\n"
failed_end:
.section .bss
.align 16
utsname: .skip 390
.section .note.GNU-stack,"",@progbits
