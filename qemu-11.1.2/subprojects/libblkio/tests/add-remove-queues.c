// SPDX-License-Identifier: (MIT OR Apache-2.0)
#include "util.h"

enum {
    TEST_FILE_SIZE = 2 * 1024 * 1024,
};

static char filename[] = "add-remove-queues-XXXXXX";

int main(int argc, char **argv)
{
    struct test_opts opts;
    struct blkio *b;
    bool can_add_queues;
    int max_queues;

    parse_generic_opts(&opts, argc, argv);

    create_and_connect(&b, &opts, filename, TEST_FILE_SIZE);

    ok(blkio_get_bool(b, "can-add-queues", &can_add_queues));

    if (!can_add_queues) {
        ok(blkio_set_int(b, "num-queues", 0));
        ok(blkio_set_int(b, "num-poll-queues", 0));
        err(blkio_start(b), -EINVAL);
    }

    err(blkio_add_queue(b), can_add_queues ? -EBUSY : -ENOTSUP);

    ok(blkio_get_int(b, "max-queues", &max_queues));
    skip_if(max_queues < 2);

    ok(blkio_set_int(b, "num-queues", 2));
    ok(blkio_start(b));

    assert(blkio_get_queue(b, 0));
    assert(blkio_get_queue(b, 1));
    assert(!blkio_get_queue(b, 2));
    assert(!blkio_get_queue(b, 3));

    ok(blkio_remove_queue(b, 0));
    err(blkio_remove_queue(b, 0), -ENOENT);

    assert(!blkio_get_queue(b, 0));
    assert(blkio_get_queue(b, 1));
    assert(!blkio_get_queue(b, 2));
    assert(!blkio_get_queue(b, 3));

    if (can_add_queues) {
        assert(blkio_add_queue(b) == 0);
        assert(blkio_add_queue(b) == 2);

        assert(blkio_get_queue(b, 0));
        assert(blkio_get_queue(b, 1));
        assert(blkio_get_queue(b, 2));
        assert(!blkio_get_queue(b, 3));

        ok(blkio_remove_queue(b, 0));

        assert(!blkio_get_queue(b, 0));
        assert(blkio_get_queue(b, 1));
        assert(blkio_get_queue(b, 2));
        assert(!blkio_get_queue(b, 3));

        err(blkio_remove_queue(b, 0), -ENOENT);
        ok(blkio_remove_queue(b, 2));

        assert(!blkio_get_queue(b, 0));
        assert(blkio_get_queue(b, 1));
        assert(!blkio_get_queue(b, 2));
        assert(!blkio_get_queue(b, 3));
    }

    blkio_destroy(&b);
    assert(!b);

    return 0;
}
