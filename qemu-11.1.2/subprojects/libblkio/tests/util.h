// SPDX-License-Identifier: (MIT OR Apache-2.0)
/* Testing utility functions */

#ifndef TESTS_UTIL_H
#define TESTS_UTIL_H

#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <assert.h>
#include <sys/types.h>
#include <unistd.h>
#include "blkio.h"

/* Check that expr is 0 */
#define ok(expr) \
    do { \
        int ret = (expr); \
        if (ret != 0) { \
            fprintf(stderr, "%s failed (ret %d): %s\n", #expr, ret, \
                    blkio_get_error_msg()); \
            abort(); \
        } \
    } while (0)

/* Check that expr equals expected and that an error message was set. */
#define err(expr, expected) \
    do { \
        int ret = (expr); \
        if (ret != (expected)) { \
            fprintf(stderr, "%s expected return value %d, got %d\n", \
                    #expr, (expected), ret); \
            abort(); \
        } \
        assert(blkio_get_error_msg()[0] != '\0'); \
    } while (0)

/* Terminate test with exit code 77, which Meson interprets as skipped. */
#define skip() exit(77)

/* If condition is true, skip the test. */
#define skip_if(condition) \
    do { \
        if (condition) \
            skip(); \
    } while (0)

struct test_opts {
    char *driver;
    char *path;
};

void parse_generic_opts(struct test_opts *opts, int argc, char **argv);
void register_cleanup(void (*fn)(void));
int create_file(char *namebuf, off_t length);

void create(struct blkio **b, struct test_opts *opts, char *filename,
            size_t file_size);
void create_and_connect(struct blkio **b, struct test_opts *opts,
                        char *filename, size_t file_size);

bool driver_is_io_uring(char *driver);
bool driver_is_virtio_blk(char *driver);
bool driver_is_virtio_blk_vhost_vdpa(char *driver);

// pread_full() and pwrite_full() retry on short read/write. pread_full() may
// still succeed and return less that count when EOF is reached.
ssize_t pread_full(int fd, void *buf, size_t count, off_t offset);
ssize_t pwrite_full(int fd, const void *buf, size_t count, off_t offset);

#endif /* TESTS_UTIL_H */
