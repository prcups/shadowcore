// SPDX-License-Identifier: (MIT OR Apache-2.0)

#include <getopt.h>
#include <signal.h>
#include "util.h"

static void (*cleanup)(void);

static void signal_handler(int signo)
{
    cleanup();
}

/* Call fn() at exit or when the process aborts */
void register_cleanup(void (*fn)(void))
{
    struct sigaction sigact = {
        .sa_handler = signal_handler,
        .sa_flags = SA_RESETHAND,
    };

    cleanup = fn;

    sigemptyset(&sigact.sa_mask);
    sigaction(SIGABRT, &sigact, NULL);

    atexit(fn);
}

/* Like mkstemp(3) except it also sets the file size */
int create_file(char *namebuf, off_t length)
{
    int fd = mkstemp(namebuf);
    assert(fd >= 0);

    assert(ftruncate(fd, length) == 0);

    return fd;
}

static char *cleanup_filename;

static void cleanup_file(void)
{
    if (cleanup_filename) {
        unlink(cleanup_filename);
        free(cleanup_filename);
        cleanup_filename = NULL;
    }
}

void create(struct blkio **b, struct test_opts *opts, char *filename,
            size_t file_size)
{
    const char *path;

    ok(blkio_create(opts->driver, b));
    assert(*b);

    if (opts->path) {
        path = opts->path;
    } else {
        int fd;

        cleanup_filename = strdup(filename);
        assert(cleanup_filename);

        register_cleanup(cleanup_file);
        fd = create_file(cleanup_filename, file_size);
        assert(close(fd) == 0);

        /* mkstemp manipulates the string, so let's copy it back */
        strcpy(filename, cleanup_filename);

        path = filename;
    }

    ok(blkio_set_str(*b, "path", path));
}

void create_and_connect(struct blkio **b, struct test_opts *opts,
                        char *filename, size_t file_size)
{
    create(b, opts, filename, file_size);

    ok(blkio_connect(*b));
}

static const char optstring[] = "d:p:";
static const struct option longopts[] = {
    {
        .name = "driver",
        .has_arg = required_argument,
        .val = 'd',
    },
    {
        .name = "path",
        .has_arg = required_argument,
        .val = 'p',
    },
    {
        .name = "help",
        .has_arg = no_argument,
        .val = '?',
    },
    {},
};

static void usage(char *exe_name)
{
    fprintf(stderr, "Usage: %s [--help] --driver=<name>\n"
            "\n"
            "Options:\n"
            "  --help                     Print this help message\n"
            "  -d | --driver <name>       Driver name to use in the test\n"
            "  -p | --path   <path>       File/device path to use in the test\n"
            "",
            exe_name);
    exit(EXIT_FAILURE);
}

void parse_generic_opts(struct test_opts *opts, int argc, char **argv)
{
    opts->driver = NULL;
    opts->path = NULL;

    for (;;) {
        int opt = getopt_long(argc, argv, optstring, longopts, NULL);

        if (opt == -1)
            break;

        switch (opt) {
        case 'd':
            opts->driver = optarg;
            break;
        case 'p':
            opts->path = optarg;
            break;
        case '?':
        default:
            usage(argv[0]);
        }
    }

    if (!opts->driver) {
        usage(argv[0]);
    }
}

bool driver_is_io_uring(char *driver) {
    return strcmp(driver, "io_uring") == 0;
}

bool driver_is_virtio_blk(char *driver) {
    char *virtio_blk = "virtio-blk-";

    return strncmp(driver, virtio_blk, strlen(virtio_blk)) == 0;
}

bool driver_is_virtio_blk_vhost_vdpa(char *driver) {
    return strcmp(driver, "virtio-blk-vhost-vdpa") == 0;
}

ssize_t pread_full(int fd, void *buf, size_t count, off_t offset) {
    size_t n = 0;

    while (n < count) {
        ssize_t res;

        do {
            res = pread(fd, buf + n, count - n, offset + n);
        } while (res == -1 && errno == EINTR);

        if (res < 0) {
            return res;
        }

        if (res == 0) {
            break; // reached EOF
        }

        n += res;
    }

    return n;
}

ssize_t pwrite_full(int fd, const void *buf, size_t count, off_t offset) {
    size_t n = 0;

    while (n < count) {
        ssize_t res;

        do {
            res = pwrite(fd, buf + n, count - n, offset + n);
        } while (res == -1 && errno == EINTR);

        if (res < 0) {
            return res;
        }

        assert(res != 0);

        n += res;
    }

    return n;
}
