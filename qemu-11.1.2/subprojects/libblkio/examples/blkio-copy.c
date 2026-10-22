// SPDX-License-Identifier: MIT
/*
 * blkio-copy - copy data between block devices or files
 *
 * This program copies data between libblkio block devices and/or files.
 *
 *   blkio-copy --input-blkio=<driver> <property>=<value> ... |
 *              --input-file=<path>
 *              --output-blkio=<driver> <property>=<value> ... |
 *              --output-file=<path>
 *
 * The --input-blkio=<driver> and --output-blkio=<driver> options create a
 * blkio instance with the given driver and property assignments listed on the
 * command-line. For example, the io_uring driver is opened as follows:
 *
 *   --blkio=io_uring path=/path/to/file
 *
 * The --input-file=<path> and --output-file=<path> options open a file
 * natively without libblkio. This is useful for accessing files that are not
 * multiples of a block size because they are not block devices or disk images.
 * If the input file length is not a multiple of the block size the final block
 * will be padded with zeroes.
 */
#include <blkio.h>
#include <errno.h>
#include <fcntl.h>
#include <getopt.h>
#include <inttypes.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

static uint64_t get_capacity(struct blkio *b, int fd)
{
    if (b) {
        uint64_t capacity;
        int ret;

        ret = blkio_get_uint64(b, "capacity", &capacity);
        if (ret != 0) {
            fprintf(stderr, "Failed to get capacity: %s\n", blkio_get_error_msg());
            exit(EXIT_FAILURE);
        }

        return capacity;
    } else {
        struct stat st;

        if (fstat(fd, &st) != 0) {
            perror("fstat");
            exit(EXIT_FAILURE);
        }

        return st.st_size;
    }
}

static void do_read(struct blkioq *q, int fd, uint64_t offset, uint64_t size,
                    void *buf)
{
    if (q) {
        struct blkio_completion completion;
        int ret;

        blkioq_read(q, offset, buf, size, NULL, 0);

        ret = blkioq_do_io(q, &completion, 1, 1, NULL);
        if (ret != 1) {
            fprintf(stderr, "blkioq_do_io failed: %s: %s\n", strerror(-ret),
                    blkio_get_error_msg());
            exit(EXIT_FAILURE);
        }

        if (completion.ret != 0) {
            fprintf(stderr, "Unexpected read completion.ret value %d\n",
                    completion.ret);
            exit(EXIT_FAILURE);
        }
    } else {
        ssize_t ret = pread(fd, buf, size, offset);

        if (ret < 0) {
            perror("pread");
            exit(EXIT_FAILURE);
        }

        /* Zero pad short reads */
        if (ret < size) {
            memset(buf + ret, 0, size - ret);
        }
    }
}

static void do_write(struct blkioq *q, int fd, uint64_t offset, uint64_t size,
                     const void *buf)
{
    if (q) {
        struct blkio_completion completion;
        int ret;

        blkioq_write(q, offset, buf, size, NULL, 0);

        ret = blkioq_do_io(q, &completion, 1, 1, NULL);
        if (ret != 1) {
            fprintf(stderr, "blkioq_do_io failed: %s: %s\n", strerror(-ret),
                    blkio_get_error_msg());
            exit(EXIT_FAILURE);
        }

        if (completion.ret != 0) {
            fprintf(stderr, "Unexpected write completion.ret value %d\n",
                    completion.ret);
            exit(EXIT_FAILURE);
        }
    } else {
        ssize_t ret = pwrite(fd, buf, size, offset);

        if (ret < 0) {
            perror("pwrite");
            exit(EXIT_FAILURE);
        }
    }
}

static void copy(struct blkio *input_blkio, int input_fd,
                 struct blkio *output_blkio, int output_fd)
{
    uint64_t remaining = get_capacity(input_blkio, input_fd);
    uint64_t block_size = 128 * 1024;
    void *input_buf = NULL;
    void *output_buf = NULL;
    struct blkioq *input_q = NULL;
    struct blkioq *output_q = NULL;
    struct blkio_mem_region input_mem_region;
    struct blkio_mem_region output_mem_region;
    int ret;

    if (input_blkio) {
        ret = blkio_alloc_mem_region(input_blkio, &input_mem_region,
                                     block_size);
        if (ret < 0) {
            fprintf(stderr, "blkio_alloc_mem_region: %s\n",
                    blkio_get_error_msg());
            exit(EXIT_FAILURE);
        }

        ret = blkio_map_mem_region(input_blkio, &input_mem_region);
        if (ret < 0) {
            fprintf(stderr, "blkio_map_mem_region: %s\n",
                    blkio_get_error_msg());
            exit(EXIT_FAILURE);
        }

        output_buf = input_buf = input_mem_region.addr;
        input_q = blkio_get_queue(input_blkio, 0);
    }

    if (output_blkio) {
        ret = blkio_alloc_mem_region(output_blkio, &output_mem_region,
                                     block_size);
        if (ret < 0) {
            fprintf(stderr, "blkio_alloc_mem_region: %s\n",
                    blkio_get_error_msg());
            exit(EXIT_FAILURE);
        }

        ret = blkio_map_mem_region(output_blkio, &output_mem_region);
        if (ret < 0) {
            fprintf(stderr, "blkio_map_mem_region: %s\n",
                    blkio_get_error_msg());
            exit(EXIT_FAILURE);
        }

        output_buf = output_mem_region.addr;
        if (!input_buf) {
            input_buf = output_buf;
        }
        output_q = blkio_get_queue(output_blkio, 0);
    }

    for (uint64_t offset = 0; offset < remaining; offset += block_size) {
        do_read(input_q, input_fd, offset, block_size, input_buf);

        if (input_buf != output_buf) {
            memcpy(output_buf, input_buf, block_size);
        }

        do_write(output_q, output_fd, offset, block_size, output_buf);
    }

    /*
     * No need to call blkio_unmap_mem_region() or blkio_free_mem_region() since
     * we're terminating and blkio_destroy() will do that for us.
     */
}

static void usage(const char *progname)
{
    printf("Usage: %s --input-blkio=<driver> <property>=<value> ... |\n", progname);
    printf("          --input-file=<path>\n");
    printf("          --output-blkio=<driver> <property>=<value> ... |\n");
    printf("          --output-file=<path>\n");
    printf("Copy data between blkio block devices and/or files.\n");
    exit(EXIT_FAILURE);
}

static void assign_property(struct blkio *b,
                            const char *property,
                            const char *value)
{
    int ret;

    ret = blkio_set_str(b, property, value);
    if (ret < 0) {
        fprintf(stderr, "Failed to assign \"%s\" to \"%s\": %s: %s\n",
                value, property, strerror(-ret), blkio_get_error_msg());
        exit(EXIT_FAILURE);
    }
}

static void handle_property_option(struct blkio *b, char *option)
{
    char *property = option;
    char *value = strchr(property, '=');

    if (!value) {
        fprintf(stderr, "%s: missing property value\n", option);
        exit(EXIT_FAILURE);
    }

    /* The C standard allows argv[] string modification */
    *value = '\0';
    value++;

    assign_property(b, property, value);
}

static void connect_and_start(struct blkio *b)
{
    if (blkio_connect(b) != 0) {
        fprintf(stderr, "Unable to connect: %s\n", blkio_get_error_msg());
        exit(EXIT_FAILURE);
    }

    if (blkio_start(b) != 0) {
        fprintf(stderr, "Unable to start: %s\n", blkio_get_error_msg());
        exit(EXIT_FAILURE);
    }
}

enum {
    OPT_PROPERTY = 1, /* getopt_long(3) non-options with optstring "-" */
    OPT_INPUT_BLKIO = 256,
    OPT_INPUT_FILE = 257,
    OPT_OUTPUT_BLKIO = 258,
    OPT_OUTPUT_FILE = 259,
};

static const struct option options[] = {
    {
        .name = "help",
        .has_arg = no_argument,
        .flag = NULL,
        .val = 'h',
    },
    {
        .name = "input-blkio",
        .has_arg = required_argument,
        .flag = NULL,
        .val = OPT_INPUT_BLKIO,
    },
    {
        .name = "input-file",
        .has_arg = required_argument,
        .flag = NULL,
        .val = OPT_INPUT_FILE,
    },
    {
        .name = "output-blkio",
        .has_arg = required_argument,
        .flag = NULL,
        .val = OPT_OUTPUT_BLKIO,
    },
    {
        .name = "output-file",
        .has_arg = required_argument,
        .flag = NULL,
        .val = OPT_OUTPUT_FILE,
    },
    {}
};

int main(int argc, char **argv)
{
    struct blkio *input_blkio = NULL;
    int input_fd = -1;
    struct blkio *output_blkio = NULL;
    int output_fd = -1;
    int opt;
    int ret;

    while ((opt = getopt_long(argc, argv, "-h", options, NULL)) != -1) {
        switch (opt) {
        case 'h':
            usage(argv[0]);
            break;
        case OPT_INPUT_BLKIO:
            if (input_blkio || input_fd != -1) {
                fprintf(stderr, "Only one --input-* option is allowed\n");
                usage(argv[0]);
            }

            ret = blkio_create(optarg, &input_blkio);
            if (ret != 0) {
                fprintf(stderr, "--input-blkio=%s: %s\n", optarg,
                        blkio_get_error_msg());
                usage(argv[0]);
            }
            break;
        case OPT_INPUT_FILE:
            if (input_blkio || input_fd != -1) {
                fprintf(stderr, "Only one --input-* option is allowed\n");
                usage(argv[0]);
            }

            input_fd = open(optarg, O_RDONLY);
            if (input_fd < 0) {
                fprintf(stderr, "--input-file=%s: %m\n", optarg);
                usage(argv[0]);
            }
            break;
        case OPT_OUTPUT_BLKIO:
            if (!input_blkio && input_fd == -1) {
                fprintf(stderr, "--output-* must occur after an --input-* option\n");
                usage(argv[0]);
            }
            if (output_blkio || output_fd != -1) {
                fprintf(stderr, "Only one --output-* option is allowed\n");
                usage(argv[0]);
            }

            ret = blkio_create(optarg, &output_blkio);
            if (ret != 0) {
                fprintf(stderr, "--output-blkio=%s: %s\n", optarg,
                        blkio_get_error_msg());
                usage(argv[0]);
            }
            break;
        case OPT_OUTPUT_FILE:
            if (!input_blkio && input_fd == -1) {
                fprintf(stderr, "--output-* must occur after an --input-* option\n");
                usage(argv[0]);
            }
            if (output_blkio || output_fd != -1) {
                fprintf(stderr, "Only one --output-* option is allowed\n");
                usage(argv[0]);
            }

            output_fd = open(optarg, O_WRONLY);
            if (output_fd < 0) {
                fprintf(stderr, "--output-file=%s: %m\n", optarg);
                usage(argv[0]);
            }
            break;
        case OPT_PROPERTY:
            if (output_blkio) {
                handle_property_option(output_blkio, optarg);
            } else if (input_blkio && output_fd == -1) {
                handle_property_option(input_blkio, optarg);
            } else {
                fprintf(stderr, "unexpected option: %s\n", optarg);
                usage(argv[0]);
            }
            break;
        default:
            usage(argv[0]);
            break;
        }
    }

    while (optind < argc) {
        if (!output_blkio) {
            fprintf(stderr, "unexpected option: %s\n", argv[optind]);
            usage(argv[0]);
        }

        handle_property_option(output_blkio, argv[optind++]);
    }

    if (!input_blkio && input_fd == -1) {
        fprintf(stderr, "Missing --input-* option\n");
        usage(argv[0]);
    }

    if (!output_blkio && output_fd == -1) {
        fprintf(stderr, "Missing --output-* option\n");
        usage(argv[0]);
    }

    if (!input_blkio && !output_blkio) {
        /* blkio_alloc_mem_region() is called so we need a blkio instance */
        fprintf(stderr, "One of --input-blkio and --output-blkio must be given\n");
        usage(argv[0]);
    }

    if (input_blkio) {
        connect_and_start(input_blkio);
    }
    if (output_blkio) {
        connect_and_start(output_blkio);
    }

    /*
     * If the program was more complex it would be nice to abstract blkio vs fd
     * I/O instead of passing around both values everywhere, but it's alright
     * for a small program.
     */
    copy(input_blkio, input_fd, output_blkio, output_fd);

    if (input_fd != -1) {
        close(input_fd);
    }
    if (output_fd != -1) {
        close(output_fd);
    }

    if (input_blkio) {
        blkio_destroy(&input_blkio);
    }
    if (output_blkio) {
        blkio_destroy(&output_blkio);
    }
    return EXIT_SUCCESS;
}
