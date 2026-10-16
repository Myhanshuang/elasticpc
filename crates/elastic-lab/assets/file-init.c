/* Laboratory PID 1 only. Module loading and /dev/vda I/O run INSIDE the Guest. */
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mount.h>
#include <sys/reboot.h>
#include <sys/syscall.h>
#include <unistd.h>
#define TOKEN "@TOKEN@"
static void fail(const char *step) {
    printf("ELASTICPC_DISK_FAIL:%s:%s\n", TOKEN, step);
    fflush(stdout);
    reboot(RB_POWER_OFF);
    _exit(1);
}
int main(void) {
    setbuf(stdout, NULL);
    if (getpid() != 1) return 2;
    if (mount("devtmpfs", "/dev", "devtmpfs", 0, NULL)) fail("devtmpfs");
    int module = open("/virtio_blk.ko", O_RDONLY);
    if (module < 0 || syscall(SYS_finit_module, module, "", 0)) fail("module");
    close(module);
    int fd = open("/dev/vda", O_RDWR | O_SYNC);
    if (fd < 0) fail("open");
    unsigned char actual[512], expected[512];
    memset(expected, 0, sizeof expected);
    memcpy(expected, TOKEN, strlen(TOKEN));
    if (pread(fd, actual, sizeof actual, 0) != sizeof actual) fail("read");
    int wrote = memcmp(actual, expected, sizeof actual) != 0;
    if (wrote) {
        for (unsigned i=0; i<sizeof actual; ++i) if (actual[i] != 0x11) fail("unexpected-data");
        if (pwrite(fd, expected, sizeof expected, 0) != sizeof expected || fsync(fd)) fail("write");
        if (pread(fd, actual, sizeof actual, 0) != sizeof actual || memcmp(actual, expected, sizeof actual)) fail("readback");
        printf("ELASTICPC_DISK_WRITE:%s\n", TOKEN);
    } else printf("ELASTICPC_DISK_PERSISTED:%s\n", TOKEN);
    close(fd);
    printf("ELASTICPC_DISK_READY:%s\n", TOKEN);
    for (;;) {
        int c = getchar();
        if (c == 'h') {
            printf("ELASTICPC_DISK_%s:%s\n", wrote ? "WRITE" : "PERSISTED", TOKEN);
            printf("ELASTICPC_DISK_READY:%s\n", TOKEN);
        }
        if (c == 'q') {
            sync();
            printf("ELASTICPC_DISK_POWEROFF:%s\n", TOKEN);
            reboot(RB_POWER_OFF);
            fail("poweroff");
        }
        if (c == EOF) pause();
    }
}
