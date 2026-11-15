/* Synthetic-memory-only optional TSK candidate probe. No image path is accepted.
 * Compile against the explicitly selected installed header/library pair; this
 * is a dependency qualification experiment, not the filesystem adapter.
 */
#include <tsk/libtsk.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

typedef struct {
    TSK_IMG_INFO base;
    uint64_t reads;
    uint64_t requested_bytes;
    uint64_t max_read;
    int invalid_request;
} ProbeImage;

static ssize_t probe_read(TSK_IMG_INFO *image, TSK_OFF_T offset, char *buffer, size_t length) {
    ProbeImage *probe = (ProbeImage *)image;
    probe->reads++;
    if (offset < 0 || (uint64_t)offset > (uint64_t)image->size || length > (uint64_t)image->size - (uint64_t)offset || length > 1024 * 1024) {
        probe->invalid_request = 1;
        return -1;
    }
    probe->requested_bytes += length;
    if (length > probe->max_read) probe->max_read = length;
    for (size_t i = 0; i < length; i++) buffer[i] = (char)(((uint64_t)offset + i) % 251);
    return (ssize_t)length;
}
static void probe_close(TSK_IMG_INFO *image) { free(image); }
static void probe_stat(TSK_IMG_INFO *image, FILE *output) { (void)image; (void)output; }
int main(void) {
    printf("header_version\t%s\nruntime_version\t%s\n", TSK_VERSION_STR, tsk_version_get_str());
    if (strcmp(TSK_VERSION_STR, tsk_version_get_str()) != 0) {
        fprintf(stderr, "header/runtime mismatch: adapter must remain disabled\n");
        return 2;
    }
    ProbeImage *probe = calloc(1, sizeof(*probe));
    if (!probe) return 3;
    TSK_IMG_INFO *image = tsk_img_open_external(probe, 128 * 1024, 512, probe_read, probe_close, probe_stat);
    if (!image) { free(probe); return 4; }
    char buffer[31];
    ssize_t count = tsk_img_read(image, 513, buffer, sizeof(buffer));
    int valid = count == (ssize_t)sizeof(buffer);
    for (size_t i = 0; valid && i < sizeof(buffer); i++) valid = (unsigned char)buffer[i] == (513 + i) % 251;
    printf("sizeof_img_info\t%zu\nsizeof_attr_run\t%zu\nread_offset_units\tbytes\n", sizeof(TSK_IMG_INFO), sizeof(TSK_FS_ATTR_RUN));
    printf("callback_reads\t%llu\ncallback_bytes\t%llu\nmax_callback_read\t%llu\n", (unsigned long long)probe->reads, (unsigned long long)probe->requested_bytes, (unsigned long long)probe->max_read);
    printf("byte_pattern_check\t%s\nrange_violation\t%s\n", valid ? "pass" : "fail", probe->invalid_request ? "true" : "false");
    valid = valid && !probe->invalid_request && probe->reads > 0;
    tsk_img_close(image);
    return valid ? 0 : 5;
}
