// SPDX-License-Identifier: (MIT OR Apache-2.0)
#include <string.h>
#include "util.h"

int main(int argc, char **argv)
{
    struct test_opts opts;
    struct blkio *b;
    char *driver = NULL;

    parse_generic_opts(&opts, argc, argv);

    ok(blkio_create(opts.driver, &b));
    assert(b);

    ok(blkio_get_str(b, "driver", &driver));
    assert(driver);
    assert(strcmp(driver, opts.driver) == 0);
    free(driver);
    driver = NULL;

    err(blkio_set_str(b, "driver", "foo"), -EACCES);

    /* Check driver has not changed */
    ok(blkio_get_str(b, "driver", &driver));
    assert(driver);
    assert(strcmp(driver, opts.driver) == 0);
    free(driver);

    blkio_destroy(&b);
    assert(!b);

    return 0;
}

