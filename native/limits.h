#ifndef FRAMEHARBOR_NATIVE_LIMITS_H
#define FRAMEHARBOR_NATIVE_LIMITS_H
/* Shared limits and trusted-parent IPC for optional native helpers. These do
 * not constitute an exploitation-resistant operating-system security sandbox. */
#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/resource.h>
#include <unistd.h>
#ifdef __APPLE__
#include <mach/mach.h>
#endif
#ifndef FRAMEHARBOR_TEST_MODE
#define FRAMEHARBOR_TEST_MODE 0
#endif
static inline int fh_parse_u64(const char *text, uint64_t *result) {
    if (!text[0]) return 0;
    uint64_t n = 0;
    for (const unsigned char *s = (const unsigned char *)text; *s; s++) {
        if (*s < '0' || *s > '9' || n > (UINT64_MAX - (*s - '0')) / 10) return 0;
        n = n * 10 + (*s - '0');
    }
    *result = n;
    return 1;
}
static inline int fh_limit_resource(int resource, rlim_t soft, rlim_t hard) {
    struct rlimit limit = {soft, hard};
    if (setrlimit(resource, &limit) == 0) return 1;
    fprintf(stderr, "resource-limit-failed %d errno %d\n", resource, errno);
    return 0;
}
static inline int fh_limit_memory(void) {
#ifdef __APPLE__
    /* Darwin compares DATA/AS with the whole already loaded VM map. Limit
     * additional mappings, not fictitious absolute resident memory. */
    mach_task_basic_info_data_t info;
    mach_msg_type_number_t count = MACH_TASK_BASIC_INFO_COUNT;
    if (task_info(mach_task_self(), MACH_TASK_BASIC_INFO, (task_info_t)&info, &count) != KERN_SUCCESS || info.virtual_size > RLIM_INFINITY - 64 * 1024 * 1024) return 0;
    rlim_t ceiling = info.virtual_size + 64 * 1024 * 1024;
    fprintf(stderr, "virtual-mapping-limit baseline %llu additional %u\n", (unsigned long long)info.virtual_size, 64U * 1024U * 1024U);
    return fh_limit_resource(RLIMIT_DATA, ceiling, ceiling) && fh_limit_resource(RLIMIT_AS, ceiling, ceiling);
#else
    return fh_limit_resource(RLIMIT_DATA, 64 * 1024 * 1024, 64 * 1024 * 1024) && fh_limit_resource(RLIMIT_RSS, 64 * 1024 * 1024, 64 * 1024 * 1024);
#endif
}
static inline int fh_apply_limits(void) {
    return fh_limit_resource(RLIMIT_CORE, 0, 0) && fh_limit_resource(RLIMIT_FSIZE, 0, 0) && fh_limit_resource(RLIMIT_CPU, 30, 30) && fh_limit_resource(RLIMIT_NOFILE, 32, 32) && fh_limit_memory();
}
static inline int fh_max_rss_bytes(uint64_t *bytes) {
    struct rusage usage;
    if (getrusage(RUSAGE_SELF, &usage) != 0) return 0;
#ifdef __APPLE__
    *bytes = (uint64_t)usage.ru_maxrss;
#else
    *bytes = (uint64_t)usage.ru_maxrss * 1024;
#endif
    return 1;
}
static inline int fh_receive_bytes(void *buffer, size_t length) {
    char response[64], extra;
    size_t received = 0;
    if (!fgets(response, sizeof(response), stdin) || sscanf(response, "OK %zu %c", &received, &extra) != 1 || received != length || fread(buffer, 1, length, stdin) != length) return 0;
#if FRAMEHARBOR_TEST_MODE == 1
    for (;;) pause();
#elif FRAMEHARBOR_TEST_MODE == 2
    _exit(77);
#elif FRAMEHARBOR_TEST_MODE == 3
    printf("INVALID-PROTOCOL\n"); fflush(stdout); _exit(0);
#endif
    return 1;
}
#endif
