// SPDX-License-Identifier: MIT
/*
 * blkio-bench - block I/O benchmark
 *
 * This program measures performance statistics for a given I/O workload. This
 * is an example program that demonstrates multi-threaded event-driven or polled
 * I/O. For real disk I/O benchmarking, use fio(1).
 *
 *   blkio-bench [--num-threads=<n>]  (default: 1)
 *               [--iodepth=<n>]  (default: 1)
 *               [--readwrite=read|write|randread|randwrite]  (default: read)
 *               [--blocksize=<bytes>]  (default: 4096)
 *               [--runtime=<seconds>]  (default: 30)
 *               [--poll]
 *               <driver> <property>=<value> ...
 */
#include <assert.h>
#include <blkio.h>
#include <errno.h>
#include <getopt.h>
#include <inttypes.h>
#include <limits.h>
#include <pthread.h>
#include <stdatomic.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <stdnoreturn.h>
#include <string.h>
#include <sys/epoll.h>
#include <sys/eventfd.h>
#include <sys/syscall.h>
#include <time.h>
#include <unistd.h>

#define GiB (1ULL<<30)

/* Read/write access types */
enum readwrite {
    RW_READ = 0,
    RW_WRITE,
    RW_RANDREAD,
    RW_RANDWRITE,
};

struct config {
    unsigned num_threads;
    unsigned iodepth;
    enum readwrite readwrite;
    unsigned blocksize_bytes;
    unsigned runtime_secs;
    uint64_t capacity;
    bool poll;
};

struct io_stats {
    uint64_t count;
    double runtime_secs;
    uint64_t min_latency_ns;
    uint64_t max_latency_ns;
    uint64_t total_latency_ns;
    uint64_t calls;  /* with max_completions > 0 */
    unsigned min_completions;
    unsigned max_completions;
};

struct io_request {
    struct timespec start;
    void *buf;
};

struct thread_data {
    pthread_t thread;

    const struct config *config;

    struct io_request *requests;
    struct blkio_completion *completions;

    /* Requests ready to start. */
    struct io_request **pending;
    unsigned pending_count;

    void *full_buf; /* for all requests in this thread */

    /* Blocking eventfds used to synchronize with the main thread */
    int init_fd;
    int start_fd;
    int stop_fd;

    /* Used to notify thread to stop when polling */
    atomic_bool *should_stop;

    struct blkioq *q;
    uint64_t offset;
    struct io_stats stats;

    struct random_data random_buf;
    char random_state[256];
};

static void gettime(struct timespec *now)
{
    if (clock_gettime(CLOCK_MONOTONIC, now) != 0) {
        perror("clock_gettime");
        exit(EXIT_FAILURE);
    }
}

static uint64_t delta_ns(const struct timespec *start,
                         const struct timespec *end)
{
    static const uint64_t ns_per_sec = 1000000000ul;

    return (end->tv_sec - start->tv_sec) * ns_per_sec +
           end->tv_nsec - start->tv_nsec;
}

static void prepare_request(struct io_request *req, struct thread_data *td,
                        const struct timespec *now)
{
    enum readwrite readwrite = td->config->readwrite;
    uint64_t offset;
    unsigned blocksize_bytes = td->config->blocksize_bytes;

    req->start = *now;

    if (readwrite == RW_READ || readwrite == RW_WRITE) {
        offset = td->offset;
        td->offset = (td->offset + blocksize_bytes) % td->config->capacity;
    } else {
        int32_t val32;
        uint64_t val;

        random_r(&td->random_buf, &val32);
        val = val32;
        random_r(&td->random_buf, &val32);
        val |= (uint64_t)val32 << 32;

        offset = (val * blocksize_bytes) % td->config->capacity;
    }

    if (readwrite == RW_READ || readwrite == RW_RANDREAD) {
        blkioq_read(td->q, offset, req->buf, blocksize_bytes, req, 0);
    } else {
        blkioq_write(td->q, offset, req->buf, blocksize_bytes, req, 0);
    }
}

static void submit_requests(struct thread_data *td)
{
    int n;

    n = blkioq_do_io(td->q, NULL, 0, 0, NULL);
    if (n < 0) {
        fprintf(stderr, "Failed to submit I/O requests: %s: %s\n",
                strerror(-n), blkio_get_error_msg());
        exit(EXIT_FAILURE);
    }
}

static int submit_requests_and_reap_completions(struct thread_data *td)
{
    int n;

    td->stats.calls++;
    n = blkioq_do_io(td->q, td->completions, 0, td->config->iodepth, NULL);
    if (n < 0) {
        fprintf(stderr, "Failed to submit and reap: %s: %s\n",
                strerror(-n), blkio_get_error_msg());
        exit(EXIT_FAILURE);
    }

    return n;
}

static void process_completions(struct thread_data *td, int n)
{
    struct timespec now;

    gettime(&now);

    if (n < td->stats.min_completions) {
        td->stats.min_completions = n;
    }
    if (n > td->stats.max_completions) {
        td->stats.max_completions = n;
    }

    for (int i = 0; i < n; i++) {
        struct io_request *req = td->completions[i].user_data;
        uint64_t latency_ns;

        if (td->completions[i].ret != 0) {
            fprintf(stderr, "I/O request failed: %d\n",
                    td->completions[i].ret);
            exit(EXIT_FAILURE);
        }

        td->stats.count++;

        latency_ns = delta_ns(&req->start, &now);
        if (latency_ns < td->stats.min_latency_ns) {
            td->stats.min_latency_ns = latency_ns;
        }
        if (latency_ns > td->stats.max_latency_ns) {
            td->stats.max_latency_ns = latency_ns;
        }
        td->stats.total_latency_ns += latency_ns;

        /* Schedule for starting next request. */
        td->pending[td->pending_count++] = req;
    }
}

static void start_pending_requests(struct thread_data *td)
{
    struct timespec now;

    gettime(&now);

    for (int i = 0; i < td->pending_count; i++) {
        prepare_request(td->pending[i], td, &now);
    }

    td->pending_count = 0;
}

void *thread_main(void *opaque)
{
    struct thread_data *td = opaque;
    struct epoll_event epoll_event = {
        .events = EPOLLIN,
    };
    struct timespec start, end;
    uint64_t val = 1;
    ssize_t ret;
    int epfd;
    int completion_fd;

    td->offset = 0;

    td->completions = malloc(sizeof(*td->completions) * td->config->iodepth);
    if (!td->completions) {
        fprintf(stderr, "Failed to allocate completions\n");
        exit(EXIT_FAILURE);
    }

    td->requests = malloc(sizeof(*td->requests) * td->config->iodepth);
    if (!td->requests) {
        fprintf(stderr, "Failed to allocated requests\n");
        exit(EXIT_FAILURE);
    }

    td->pending = malloc(sizeof(*td->pending) * td->config->iodepth);
    if (!td->pending) {
        fprintf(stderr, "Failed to allocated pending\n");
        exit(EXIT_FAILURE);
    }

    /* Initialize requests and pending. */
    for (int i = 0; i < td->config->iodepth; i++) {
        td->requests[i].buf = td->full_buf + i * td->config->blocksize_bytes;
        td->pending[i] = &td->requests[i];
    }

    td->pending_count = td->config->iodepth;

    epfd = epoll_create1(EPOLL_CLOEXEC);
    if (epfd < 0) {
        perror("epoll_create1");
        exit(EXIT_FAILURE);
    }

    blkioq_set_completion_fd_enabled(td->q, !td->config->poll);

    completion_fd = blkioq_get_completion_fd(td->q);
    epoll_event.data.u32 = completion_fd;
    if (epoll_ctl(epfd, EPOLL_CTL_ADD, completion_fd, &epoll_event) < 0) {
        perror("epoll_ctl");
        exit(EXIT_FAILURE);
    }

    epoll_event.data.u32 = td->stop_fd;
    if (epoll_ctl(epfd, EPOLL_CTL_ADD, td->stop_fd, &epoll_event) < 0) {
        perror("epoll_ctl");
        exit(EXIT_FAILURE);
    }

    /* Notify main thread that initialization is complete */
    ret = write(td->init_fd, &val, sizeof(val));
    assert(ret == sizeof(val));

    /* Wait until it's time to start */
    ret = read(td->start_fd, &val, sizeof(val));
    assert(ret == sizeof(val));

    gettime(&start);

    start_pending_requests(td);

    while (true) {
        uint64_t e;
        int n;

        if (td->config->poll) {
            bool stop = false;

            do {
                if (atomic_load_explicit(td->should_stop,
                                         memory_order_relaxed)) {
                    stop = true;
                } else {
                    n = submit_requests_and_reap_completions(td);
                }
            } while (!stop && n == 0);

            if (stop) {
                break;
            }
            process_completions(td, n);
            start_pending_requests(td);
        } else {
            submit_requests(td);

            n = epoll_wait(epfd, &epoll_event, 1, 500);
            if (n == -1) {
                if (errno != EINTR) {
                    perror("epoll_wait");
                    exit(EXIT_FAILURE);
                }
                continue;
            }

            if (n == 0) {
                /*
                 * If we did not receive a completion event in 500
                 * milliseconds, the library or driver are likey broken and the
                 * core dump can be useful for debugging the issue.
                 */
                fprintf(stderr, "Timeout receiving completion event - aborting\n");
                abort();
            }

            if (epoll_event.data.u32 == td->stop_fd) {
                n = -1;
                break;
            }

            if (read(completion_fd, &e, sizeof(e)) != sizeof(e)) {
                perror("read");
                exit(EXIT_FAILURE);
            }

            blkioq_set_completion_fd_enabled(td->q, false);

            for (;;) {
                n = submit_requests_and_reap_completions(td);
                if (n == 0) {
                    blkioq_set_completion_fd_enabled(td->q, true);

                    n = submit_requests_and_reap_completions(td);
                    if (n == 0) {
                        break;
                    }

                    blkioq_set_completion_fd_enabled(td->q, false);
                }

                process_completions(td, n);
            }

            start_pending_requests(td);
        }
    }

    gettime(&end);
    td->stats.runtime_secs = delta_ns(&start, &end) / 1e9;

    close(epfd);
    free(td->pending);
    free(td->requests);
    free(td->completions);
    return NULL;
}

static void thread_init(struct thread_data *td,
                        const struct config *config,
                        atomic_bool *should_stop,
                        void *full_buf,
                        struct blkioq *q,
                        uint64_t offset)
{
    uint64_t val;
    int ret;
    ssize_t ret2;

    td->config = config;
    td->full_buf = full_buf;
    td->q = q;
    td->offset = offset;
    memset(&td->random_buf, 0, sizeof(td->random_buf));
    initstate_r(syscall(SYS_gettid), td->random_state, sizeof(td->random_state),
                &td->random_buf);

    memset(&td->stats, 0, sizeof(td->stats));
    td->stats.min_latency_ns = UINT64_MAX;
    td->stats.min_completions = UINT_MAX;

    td->init_fd = eventfd(0, EFD_CLOEXEC);
    td->start_fd = eventfd(0, EFD_CLOEXEC);
    td->stop_fd = eventfd(0, EFD_CLOEXEC);
    if (td->init_fd < 0 || td->start_fd < 0 || td->stop_fd < 0) {
        perror("eventfd");
        exit(EXIT_FAILURE);
    }

    td->should_stop = should_stop;

    ret = pthread_create(&td->thread, NULL, thread_main, td);
    if (ret != 0) {
        errno = ret;
        perror("pthread_create");
        exit(EXIT_FAILURE);
    }

    /* Wait for the thread to initialize */
    ret2 = read(td->init_fd, &val, sizeof(val));
    assert(ret2 == sizeof(val));
}

static void thread_cleanup(struct thread_data *td)
{
    uint64_t val = 1;
    ssize_t ret;

    ret = write(td->stop_fd, &val, sizeof(val));
    assert(ret == sizeof(val));

    pthread_join(td->thread, NULL);

    close(td->init_fd);
    close(td->start_fd);
    close(td->stop_fd);
}

static void print_stats(const struct config *config,
                        const struct io_stats *stats, const char *indent)
{
    printf("%s  \"kiops\": %.3f,\n",
           indent,
           (double)stats->count / stats->runtime_secs / 1000.);
    printf("%s  \"gips\": %.3f,\n",
           indent,
           stats->count * config->blocksize_bytes / stats->runtime_secs / GiB);
    printf("%s  \"min_lat_us\": %.3f,\n",
           indent,
           (double)stats->min_latency_ns / 1000.);
    printf("%s  \"mean_lat_us\": %.3f,\n",
           indent,
           (double)stats->total_latency_ns / (double)stats->count / 1000.);
    printf("%s  \"max_lat_us\": %.3f,\n",
           indent,
           (double)stats->max_latency_ns / 1000.);
    printf("%s  \"calls\": %" PRIu64 ",\n",
           indent,
           stats->calls);
    printf("%s  \"min_completions\": %.2f,\n",
           indent,
           (double)stats->min_completions);
    printf("%s  \"mean_completions\": %.2f,\n",
           indent,
           (double)stats->count / stats->calls);
    printf("%s  \"max_completions\": %.2f\n",
           indent,
           (double)stats->max_completions);
}

static void print_results(const struct config *config,
                          const struct thread_data *tds)
{
    struct io_stats aggregate = {
        .count = 0,
        .runtime_secs = 0.0,
        .min_latency_ns = UINT64_MAX,
        .max_latency_ns = 0,
        .total_latency_ns = 0,
        .calls = 0,
        .min_completions = UINT_MAX,
        .max_completions = 0,
    };

    for (unsigned i = 0; i < config->num_threads; i++) {
        const struct thread_data *td = &tds[i];

        aggregate.count += td->stats.count;
        aggregate.runtime_secs += td->stats.runtime_secs;
        aggregate.total_latency_ns += td->stats.total_latency_ns;

        if (td->stats.min_latency_ns < aggregate.min_latency_ns)
            aggregate.min_latency_ns = td->stats.min_latency_ns;

        if (td->stats.max_latency_ns > aggregate.max_latency_ns)
            aggregate.max_latency_ns = td->stats.max_latency_ns;

        aggregate.calls += td->stats.calls;

        if (td->stats.min_completions < aggregate.min_completions)
            aggregate.min_completions = td->stats.min_completions;

        if (td->stats.max_completions > aggregate.max_completions)
            aggregate.max_completions = td->stats.max_completions;
    }

    aggregate.runtime_secs /= config->num_threads;

    printf("{\n");

    printf("  \"aggregate\": {\n");
    print_stats(config, &aggregate, "    ");
    printf("  },\n");

    printf("  \"threads\": [\n");

    for (unsigned i = 0; i < config->num_threads; i++) {
        printf("    {\n");
        print_stats(config, &tds[i].stats, "    ");
        printf("    }%s\n", i == config->num_threads - 1 ? "" : ",");
    }

    printf("  ]\n");
    printf("}\n");
}

static void usage(FILE *stream, const char *progname)
{
    fprintf(stream, "Usage: %s\n", progname);
    fprintf(stream, "          [--num-threads=<n>]  (default: 1)\n");
    fprintf(stream, "          [--iodepth=<n>]  (default: 1)\n");
    fprintf(stream, "          [--readwrite=read|write|randread|randwrite]  (default: read)\n");
    fprintf(stream, "          [--blocksize=<bytes>]  (default: 4096)\n");
    fprintf(stream, "          [--runtime=<seconds>]  (default: 30)\n");
    fprintf(stream, "          [--poll]\n");
    fprintf(stream, "          <driver> <property>=<value> ...\n");
    fprintf(stream, "\n");
    fprintf(stream, "Measure performance statistics for a given I/O workload.\n");
    fprintf(stream, "\n");
    fprintf(stream, "Results are outputted as JSON, reporting aggregate and per-thread\n");
    fprintf(stream, "throughput (in thousands of requests per second and GiB per second)\n");
    fprintf(stream, "and min/mean/max latency in microseconds\n");
    fprintf(stream, "latency (in microseconds per request).\n");
}

static noreturn void bad_usage(const char *progname, const char *error)
{
    fprintf(stderr, "%s\n", error);
    fprintf(stderr, "\n");
    usage(stderr, progname);
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

enum {
    OPT_NUM_THREADS = 256,
    OPT_IODEPTH,
    OPT_READWRITE,
    OPT_BLOCKSIZE,
    OPT_RUNTIME,
    OPT_POLL,
};

static const struct option options[] = {
    {
        .name = "help",
        .has_arg = no_argument,
        .flag = NULL,
        .val = 'h',
    },
    {
        .name = "num-threads",
        .has_arg = required_argument,
        .flag = NULL,
        .val = OPT_NUM_THREADS,
    },
    {
        .name = "iodepth",
        .has_arg = required_argument,
        .flag = NULL,
        .val = OPT_IODEPTH,
    },
    {
        .name = "readwrite",
        .has_arg = required_argument,
        .flag = NULL,
        .val = OPT_READWRITE,
    },
    {
        .name = "blocksize",
        .has_arg = required_argument,
        .flag = NULL,
        .val = OPT_BLOCKSIZE,
    },
    {
        .name = "runtime",
        .has_arg = required_argument,
        .flag = NULL,
        .val = OPT_RUNTIME,
    },
    {
        .name = "poll",
        .has_arg = no_argument,
        .flag = NULL,
        .val = OPT_POLL,
    },
    {}
};

int main(int argc, char **argv)
{
    struct config config = {
        .num_threads = 1,
        .iodepth = 1,
        .readwrite = RW_READ,
        .blocksize_bytes = 4096,
        .runtime_secs = 30,
        .poll = false,
    };
    atomic_bool should_stop;
    struct blkio *b = NULL;
    struct blkio_mem_region mem_region;
    struct thread_data *tds;
    const char *driver;
    unsigned remaining_time;
    int opt;
    int ret;

    while ((opt = getopt_long(argc, argv, "h", options, NULL)) != -1) {
        switch (opt) {
        case 'h':
            usage(stdout, argv[0]);
            return EXIT_SUCCESS;

        case OPT_NUM_THREADS: {
            unsigned long val = strtoul(optarg, NULL, 10);
            if (val > (unsigned long)UINT_MAX || val == 0) {
                bad_usage(argv[0], "Invalid number of threads");
            }
            config.num_threads = val;
        } break;

        case OPT_IODEPTH: {
            unsigned long val = strtoul(optarg, NULL, 10);
            if (val > (unsigned long)UINT_MAX || val == 0) {
                bad_usage(argv[0], "Invalid iodepth");
            }
            config.iodepth = val;
        } break;

        case OPT_READWRITE:
            if (strcmp(optarg, "read") == 0) {
                config.readwrite = RW_READ;
            } else if (strcmp(optarg, "write") == 0) {
                config.readwrite = RW_WRITE;
            } else if (strcmp(optarg, "randread") == 0) {
                config.readwrite = RW_RANDREAD;
            } else if (strcmp(optarg, "randwrite") == 0) {
                config.readwrite = RW_RANDWRITE;
            } else {
                bad_usage(argv[0], "Invalid readwrite option");
            }
            break;

        case OPT_BLOCKSIZE: {
            unsigned long val = strtoul(optarg, NULL, 10);
            if (val > (unsigned long)UINT_MAX || val == 0) {
                bad_usage(argv[0], "Invalid blocksize");
            }
            config.blocksize_bytes = val;
        } break;

        case OPT_RUNTIME: {
            unsigned long val = strtoul(optarg, NULL, 10);
            if (val > (unsigned long)UINT_MAX || val == 0) {
                bad_usage(argv[0], "Invalid runtime");
            }
            config.runtime_secs = val;
        } break;

        case OPT_POLL:
            config.poll = true;
            break;

        default:
            bad_usage(argv[0], "Unrecognized option");
        }
    }

    if (optind == argc) {
        bad_usage(argv[0], "Missing blkio driver name argument");
    }

    atomic_init(&should_stop, false);

    driver = argv[optind++];
    ret = blkio_create(driver, &b);
    if (ret < 0) {
        fprintf(stderr, "Failed to create blkio driver \"%s\": %s\n",
                driver, blkio_get_error_msg());
        return EXIT_FAILURE;
    }

    while (optind < argc) {
        handle_property_option(b, argv[optind++]);
    }

    if (config.readwrite == RW_READ || config.readwrite == RW_RANDREAD) {
        ret = blkio_set_bool(b, "read-only", true);
        if (ret < 0) {
            fprintf(stderr, "Failed to set read-only: %s: %s\n",
                    strerror(-ret), blkio_get_error_msg());
            exit(EXIT_FAILURE);
        }
    }

    if (blkio_connect(b) != 0) {
        fprintf(stderr, "Unable to connect: %s\n", blkio_get_error_msg());
        return EXIT_FAILURE;
    }

    ret = blkio_get_uint64(b, "capacity", &config.capacity);
    if (ret != 0) {
        fprintf(stderr, "Unable to get device capacity: %s\n",
                blkio_get_error_msg());
        return EXIT_FAILURE;
    }

    ret = blkio_set_int(b, "num-queues", config.num_threads);
    if (ret != 0) {
        fprintf(stderr, "Failed to set num-queues: %s\n",
                blkio_get_error_msg());
        return EXIT_FAILURE;
    }

    if (blkio_start(b) != 0) {
        fprintf(stderr, "Unable to start: %s\n", blkio_get_error_msg());
        return EXIT_FAILURE;
    }

    ret = blkio_alloc_mem_region(b, &mem_region,
            config.num_threads * config.iodepth * config.blocksize_bytes);
    if (ret != 0) {
        fprintf(stderr, "Failed to allocate I/O buffer memory: %s\n",
                blkio_get_error_msg());
        return EXIT_FAILURE;
    }

    ret = blkio_map_mem_region(b, &mem_region);
    if (ret != 0) {
        fprintf(stderr, "Failed to map memory region: %s\n",
                blkio_get_error_msg());
        return EXIT_FAILURE;
    }

    tds = malloc(config.num_threads * sizeof(tds[0]));
    if (!tds) {
        perror("malloc");
        return EXIT_FAILURE;
    }

    for (unsigned i = 0; i < config.num_threads; i++) {
        void *full_buf = mem_region.addr + i * config.iodepth *
                         config.blocksize_bytes;
        struct blkioq *q = blkio_get_queue(b, i);
        uint64_t offset = config.capacity / config.num_threads * i;

        thread_init(&tds[i], &config, &should_stop, full_buf, q, offset);
    }

    for (unsigned i = 0; i < config.num_threads; i++) {
        ssize_t ret2;
        uint64_t val = 1;

        ret2 = write(tds[i].start_fd, &val, sizeof(val));
        assert(ret2 == sizeof(val));
    }

    remaining_time = sleep(config.runtime_secs);
    if (remaining_time > 0) {
        fprintf(stderr, "Stopped early by a signal...\n");
        config.runtime_secs -= remaining_time;
    }

    atomic_store_explicit(&should_stop, true, memory_order_relaxed);

    for (unsigned i = 0; i < config.num_threads; i++) {
        thread_cleanup(&tds[i]);
    }

    print_results(&config, tds);

    free(tds);

    /*
     * No need to call blkio_unmap_mem_region() or blkio_free_mem_region() since
     * blkio_destroy() will do that for us.
     */

    blkio_destroy(&b);
    return EXIT_SUCCESS;
}
