// SPDX-License-Identifier: (MIT OR Apache-2.0)
#include "util.h"

int main(void)
{
    struct blkio *b;

    err(blkio_create("foo", &b), -ENOENT);
    assert(!b);

    return 0;
}
