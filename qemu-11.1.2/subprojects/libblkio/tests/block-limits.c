// SPDX-License-Identifier: (MIT OR Apache-2.0)
#include "util.h"

enum {
    TEST_FILE_SIZE = 2 * 1024 * 1024,
};

static char filename[] = "block-limits-XXXXXX";

static void expect_get_int_err(struct blkio *b,
                               int neg_errno,
                               const char *props[])
{
    for (int i = 0; props[i]; i++) {
        int value;

        err(blkio_get_int(b, props[i], &value), neg_errno);
    }
}

static void expect_get_int_ok(struct blkio *b, const char *props[])
{
    for (int i = 0; props[i]; i++) {
        int value;

        ok(blkio_get_int(b, props[i], &value));
    }
}

static void expect_set_int_err(struct blkio *b,
                               int neg_errno,
                               const char *props[])
{
    for (int i = 0; props[i]; i++) {
        err(blkio_set_int(b, props[i], 123456789), neg_errno);
    }
}

static void expect_get_uint64_err(struct blkio *b,
                                  int neg_errno,
                                  const char *props[])
{
    for (int i = 0; props[i]; i++) {
        uint64_t value;

        err(blkio_get_uint64(b, props[i], &value), neg_errno);
    }
}

static void expect_get_uint64_ok(struct blkio *b, const char *props[])
{
    for (int i = 0; props[i]; i++) {
        uint64_t value;

        ok(blkio_get_uint64(b, props[i], &value));
    }
}

static void expect_set_uint64_err(struct blkio *b,
                                  int neg_errno,
                                  const char *props[])
{
    for (int i = 0; props[i]; i++) {
        err(blkio_set_uint64(b, props[i], 123456789), neg_errno);
    }
}

/*
 * Get and set block limits properties, checking for expected error codes and
 * ignoring valid values returned.
 *
 * The specific values of these properties depend on the driver and environment
 * so it is difficult to validate them. It is still useful to exercise the code
 * just for code coverage (e.g. to prove there are no crashes).
 */
int main(int argc, char **argv)
{
    static const char *int_props[] = {
        "buf-alignment",
        "discard-alignment",
        "discard-alignment-offset",
        "max-segment-len",
        "max-segments",
        "max-transfer",
        "optimal-buf-alignment",
        "optimal-io-alignment",
        "optimal-io-size",
        "request-alignment",
        NULL
    };

    static const char *uint64_props[] = {
        "max-discard-len",
        "max-write-zeroes-len",
        NULL
    };

    struct test_opts opts;
    struct blkio *b;

    parse_generic_opts(&opts, argc, argv);

    create(&b, &opts, filename, TEST_FILE_SIZE);

    expect_get_int_err(b, -ENODEV, int_props);
    expect_get_uint64_err(b, -ENODEV, uint64_props);

    expect_set_int_err(b, -EACCES, int_props);
    expect_set_uint64_err(b, -EACCES, uint64_props);

    ok(blkio_connect(b));

    /* The properties are still read-only */
    expect_set_int_err(b, -EACCES, int_props);
    expect_set_uint64_err(b, -EACCES, uint64_props);

    expect_get_int_ok(b, int_props);
    expect_get_uint64_ok(b, uint64_props);

    blkio_destroy(&b);
    assert(!b);

    return 0;
}
