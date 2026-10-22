// SPDX-License-Identifier: (MIT OR Apache-2.0)
#include <stdbool.h>
#include <pthread.h>
#include "util.h"

static bool has_error_msg(void)
{
    return blkio_get_error_msg()[0] != '\0';
}

static void *thread_fn(void *arg) {
    struct blkio *b;

    assert(!has_error_msg());

    err(blkio_create("foo", &b), -ENOENT);
    assert(!b);

    assert(has_error_msg());

    return NULL;
}

/*
 * Verify that error messages are thread-local.
 */
int main(void)
{
    pthread_t thread;

    assert(!has_error_msg());

    assert(pthread_create(&thread, NULL, thread_fn, NULL) == 0);
    assert(pthread_join(thread, NULL) == 0);

    assert(!has_error_msg());

    return 0;
}
