// SPDX-License-Identifier: (MIT OR Apache-2.0)
#include "util.h"

enum {
    TEST_FILE_SIZE = 2 * 1024 * 1024,
};

static char filename[] = "started-XXXXXX";

int main(int argc, char **argv)
{
    struct test_opts opts;
    struct blkio *b;
    int max_queues;

    parse_generic_opts(&opts, argc, argv);

    create(&b, &opts, filename, TEST_FILE_SIZE);

    assert(blkio_get_queue(b, -1) == NULL);
    assert(blkio_get_queue(b, 0) == NULL);
    assert(blkio_get_queue(b, 1) == NULL);

    ok(blkio_connect(b));

    assert(blkio_get_queue(b, -1) == NULL);
    assert(blkio_get_queue(b, 0) == NULL);
    assert(blkio_get_queue(b, 1) == NULL);

    ok(blkio_get_int(b, "max-queues", &max_queues));
    skip_if(max_queues < 2);

    ok(blkio_set_int(b, "num-queues", 2));

    assert(blkio_get_queue(b, -1) == NULL);
    assert(blkio_get_queue(b, 0) == NULL);
    assert(blkio_get_queue(b, 1) == NULL);

    ok(blkio_start(b));

    assert(blkio_get_queue(b, 0) != NULL);
    assert(blkio_get_queue(b, 1) != NULL);
    assert(blkio_get_queue(b, 3) == NULL);
    assert(blkio_get_queue(b, 2) == NULL);
    assert(blkio_get_queue(b, 10) == NULL);
    assert(blkio_get_queue(b, -1) == NULL);

    blkio_destroy(&b);
    assert(!b);

    return 0;
}
