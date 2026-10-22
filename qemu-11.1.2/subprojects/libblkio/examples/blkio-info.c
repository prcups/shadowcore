// SPDX-License-Identifier: MIT
/*
 * blkio-info - show information about a block device
 *
 * This program prints information reported by libblkio about a block device.
 * It does this by assigning and querying the given blkio properties:
 *
 *   blkio-info [--output=text|json] <driver> <property>[=<value>] ...
 *
 * A blkio instance is created with the given driver and each property listed
 * on the command-line is either assigned or queried from left to right.
 *
 * Property assignments are initially made with the blkio instance in the
 * created state. Once the first property query is reached the blkio instance
 * is put into the connected state and all following property accesses occur in
 * the connected state.
 *
 * Output is formatted in "text" mode by default where just the property values
 * are printed in command-line argument order. In "json" mode a JSON object is
 * printed with a member for each queried property on the command-line. The
 * appropriate JSON data type is used for each property.
 *
 * Show the size of the block device, in bytes:
 *
 *   $ blkio-info io_uring path=/dev/nvme0n1 capacity
 *   107374182400
 *
 * Query the "max-queues" and "request-alignment" properties with JSON output:
 *
 *   $ blkio-info --output=json io_uring path=/dev/sdb max-queues request-alignment
 *   {
 *       "max-queues": 2147483647,
 *       "request-alignment": 512
 *   }
 */
#include <blkio.h>
#include <errno.h>
#include <getopt.h>
#include <inttypes.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* Needed for type-aware JSON output */
struct property_query_visitor
{
    void (*visit_bool)(const char *property, bool value);
    void (*visit_int)(const char *property, int value);
    void (*visit_uint64)(const char *property, uint64_t value);
    void (*visit_str)(const char *property, const char *value);
};

static void text_output_bool(const char *property, bool value)
{
    printf("%s\n", value ? "true" : "false");
}

static void text_output_int(const char *property, int value)
{
    printf("%d\n", value);
}

static void text_output_uint64(const char *property, uint64_t value)
{
    printf("%" PRId64 "\n", value);
}

static void text_output_str(const char *property, const char *value)
{
    printf("%s\n", value);
}

static const struct property_query_visitor text_output_visitor = {
    .visit_bool = text_output_bool,
    .visit_int = text_output_int,
    .visit_uint64 = text_output_uint64,
    .visit_str = text_output_str,
};

static void json_output_bool(const char *property, bool value)
{
    printf("    \"%s\": %s\n", property, value ? "true" : "false");
}

static void json_output_int(const char *property, int value)
{
    printf("    \"%s\": %d\n", property, value);
}

static void json_output_uint64(const char *property, uint64_t value)
{
    printf("    \"%s\": %" PRId64 "\n", property, value);
}

static void json_output_str(const char *property, const char *value)
{
    printf("    \"%s\": \"%s\"\n", property, value);
}

static const struct property_query_visitor json_output_visitor = {
    .visit_bool = json_output_bool,
    .visit_int = json_output_int,
    .visit_uint64 = json_output_uint64,
    .visit_str = json_output_str,
};

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

static void query_property(struct blkio *b, const char *property,
                           const struct property_query_visitor *visitor)
{
    bool bool_value;
    int int_value;
    uint64_t uint64_value;
    char *str_value;
    int ret;

    /* There is no property type information, so brute force it... */

    ret = blkio_get_bool(b, property, &bool_value);
    if (ret == 0) {
        visitor->visit_bool(property, bool_value);
        return;
    }
    if (ret != -ENOTTY) {
        goto err;
    }

    ret = blkio_get_int(b, property, &int_value);
    if (ret == 0) {
        visitor->visit_int(property, int_value);
        return;
    }
    if (ret != -ENOTTY) {
        goto err;
    }

    ret = blkio_get_uint64(b, property, &uint64_value);
    if (ret == 0) {
        visitor->visit_uint64(property, uint64_value);
        return;
    }
    if (ret != -ENOTTY) {
        goto err;
    }

    ret = blkio_get_str(b, property, &str_value);
    if (ret == 0) {
        visitor->visit_str(property, str_value);
        free(str_value);
        return;
    }

err:
    fprintf(stderr, "Failed to query \"%s\" property: %s: %s\n",
            property, strerror(-ret), blkio_get_error_msg());
    exit(EXIT_FAILURE);
}

static void usage(const char *progname)
{
    printf("Usage: %s [--output=text|json] <driver> <property>[=<value>] ...\n", progname);
    printf("Show information about a libblkio block device. This is done by\n");
    printf("assigning and querying the given blkio properties.\n\n");
    printf("  --output=text       print text output\n");
    printf("  --output=json       print JSON output\n");
    exit(EXIT_FAILURE);
}

enum {
    OPT_OUTPUT = 256,
};

static const struct option options[] = {
    {
        .name = "help",
        .has_arg = no_argument,
        .flag = NULL,
        .val = 'h',
    },
    {
        .name = "output",
        .has_arg = required_argument,
        .flag = NULL,
        .val = OPT_OUTPUT,
    },
    {}
};

int main(int argc, char **argv)
{
    const struct property_query_visitor *visitor = &text_output_visitor;
    const char *driver;
    struct blkio *b;
    bool connected = false;
    int opt;
    int ret;

    while ((opt = getopt_long(argc, argv, "h", options, NULL)) != -1) {
        switch (opt) {
        case 'h':
            usage(argv[0]);
            break;
        case OPT_OUTPUT:
            if (strcmp(optarg, "text") == 0) {
                visitor = &text_output_visitor;
            } else if (strcmp(optarg, "json") == 0) {
                visitor = &json_output_visitor;
            } else {
                fprintf(stderr, "Invalid output format \"%s\"\n\n", optarg);
                usage(argv[0]);
            }
            break;
        default:
            usage(argv[0]);
            break;
        }
    }

    if (optind == argc) {
        fprintf(stderr, "Missing driver name.\n\n");
        usage(argv[0]);
    }
    driver = argv[optind++];

    if (optind == argc) {
        fprintf(stderr, "No properties specified!\n\n");
        usage(argv[0]);
    }

    ret = blkio_create(driver, &b);
    if (ret < 0) {
        fprintf(stderr, "blkio_create: %s: %s\n", strerror(-ret),
                blkio_get_error_msg());
        return EXIT_FAILURE;
    }

    if (visitor == &json_output_visitor) {
        printf("{\n");
    }

    for (; optind < argc; optind++) {
        char *property = argv[optind];
        char *value = strchr(property, '=');

        if (value) {
            /* The C standard allows argv[] string modification */
            *value = '\0';
            value++;

            assign_property(b, property, value);
        } else {
            if (!connected) {
                if (blkio_connect(b) != 0) {
                    fprintf(stderr, "Unable to connect: %s\n",
                            blkio_get_error_msg());
                    blkio_destroy(&b);
                    return EXIT_FAILURE;
                }
                connected = true;
            }

            query_property(b, property, visitor);
        }
    }

    if (visitor == &json_output_visitor) {
        printf("}\n");
    }

    blkio_destroy(&b);
    return EXIT_SUCCESS;
}
