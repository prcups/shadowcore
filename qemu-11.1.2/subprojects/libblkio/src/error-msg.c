// SPDX-License-Identifier: (MIT OR Apache-2.0)

/*
 * Thread-local error message buffer.
 *
 * This is implemented in C with a static buffer since thread_local!() /
 * std::thread::LocalKey suffer from the problem described at
 *
 *     https://lists.gnu.org/archive/html/qemu-block/2021-04/msg00828.html
 *
 * and the only Rust equivalent to thread_local is the unstable #[thread_local]:
 *
 *     https://github.com/rust-lang/rust/issues/29594
 */

#include <stddef.h>
#include <string.h>

#define MAX_ERROR_MSG_LEN 255
static _Thread_local char error_msg_buffer[MAX_ERROR_MSG_LEN + 1] = { 0 };

const char *blkio_get_error_msg_(void)
{
    return error_msg_buffer;
}

void blkio_set_error_msg_(const char *msg, size_t msg_len)
{
    if (msg_len > MAX_ERROR_MSG_LEN)
        msg_len = MAX_ERROR_MSG_LEN;

    memcpy(error_msg_buffer, msg, msg_len);
    error_msg_buffer[msg_len] = '\0';
}
