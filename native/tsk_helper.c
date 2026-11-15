/* Narrow optional TSK 4.14.0 helper. No image pathname or descriptor is accepted.
 * Every source byte is obtained over the parent's acquisition-controlled IPC.
 * Native process isolation and RLIMITs are not an OS security sandbox.
 */
#include <tsk/libtsk.h>
#include "limits.h"
#include <inttypes.h>
#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/resource.h>
#include <sys/types.h>
#include <unistd.h>
#ifdef __APPLE__
#include <mach/mach.h>
#endif

#ifndef FRAMEHARBOR_TEST_MODE
#define FRAMEHARBOR_TEST_MODE 0
#endif
#define MAX_READ (1024U * 1024U)
#define MAX_REQUESTS 8192U
#define MAX_FILES 1024U
#define MAX_RUNS 1024U
#define MAX_ALLOCATION_CHECKS 65536U

static int failure = 0;
static uint64_t files = 0;
static uint64_t allocation_checks = 0;
typedef struct { TSK_IMG_INFO base; uint64_t reads; } ControlledImage;

static void fail(unsigned code) {
    failure = 1;
    printf("ERROR %u\n", code);
    fflush(stdout);
}
/* TSK 4.14's image cache asks for speculative 64 KiB windows. The parent
 * returns only the contiguous acquired prefix, which TSK records as cache_len.
 * A short response never authorizes the unread suffix. Zero/malformed replies
 * are fatal: do not let TSK's private unaligned-read fallback accept filler. */
static ssize_t receive_prefix(void *buffer, size_t requested) {
    char response[64], extra;
    size_t received = 0;
    if (!fgets(response, sizeof(response), stdin) || sscanf(response, "OK %zu %c", &received, &extra) != 1 || received == 0 || received > requested || fread(buffer, 1, received, stdin) != received) return -1;
#if FRAMEHARBOR_TEST_MODE == 1
    for (;;) pause();
#elif FRAMEHARBOR_TEST_MODE == 2
    _exit(77);
#elif FRAMEHARBOR_TEST_MODE == 3
    printf("INVALID-PROTOCOL\n"); fflush(stdout); _exit(0);
#endif
    return (ssize_t)received;
}
static ssize_t controlled_read(TSK_IMG_INFO *image, TSK_OFF_T offset, char *buffer, size_t length) {
    ControlledImage *controlled = (ControlledImage *)image;
    if (failure || ++controlled->reads > MAX_REQUESTS || offset < 0 || length == 0 || length > MAX_READ || (uint64_t)offset > (uint64_t)image->size || length > (uint64_t)image->size - (uint64_t)offset) {
        fail(10); _exit(10);
    }
    printf("READ %" PRIu64 " %zu\n", (uint64_t)offset, length);
    fflush(stdout);
    ssize_t received = receive_prefix(buffer, length);
    if (received < 0) {
        fail(11); _exit(11);
    }
    return received;
}
static void controlled_close(TSK_IMG_INFO *image) { free(image); }
static void controlled_stat(TSK_IMG_INFO *image, FILE *out) { (void)image; (void)out; }

typedef struct { uint64_t count; int allocated; } AllocationResult;
static TSK_WALK_RET_ENUM allocation_callback(const TSK_FS_BLOCK *block, void *opaque) {
    AllocationResult *result = opaque;
    result->count++;
    allocation_checks++;
    if (!(block->flags & TSK_FS_BLOCK_FLAG_ALLOC) || (block->flags & (TSK_FS_BLOCK_FLAG_UNALLOC | TSK_FS_BLOCK_FLAG_BAD))) result->allocated = 0;
    if (allocation_checks >= MAX_ALLOCATION_CHECKS) return TSK_WALK_STOP;
    return TSK_WALK_CONT;
}
static int allocation_consistent(TSK_FS_INFO *fs, const TSK_FS_ATTR_RUN *run) {
    if (run->flags != TSK_FS_ATTR_RUN_FLAG_NONE || run->len == 0 || run->addr >= fs->block_count || run->len > fs->block_count - run->addr || run->len > MAX_ALLOCATION_CHECKS - allocation_checks) return 2;
    AllocationResult result = {0, 1};
    uint8_t status = tsk_fs_block_walk(fs, run->addr, run->addr + run->len - 1,
        (TSK_FS_BLOCK_WALK_FLAG_ENUM)(TSK_FS_BLOCK_WALK_FLAG_ALLOC | TSK_FS_BLOCK_WALK_FLAG_UNALLOC | TSK_FS_BLOCK_WALK_FLAG_CONT | TSK_FS_BLOCK_WALK_FLAG_META | TSK_FS_BLOCK_WALK_FLAG_AONLY), allocation_callback, &result);
    if (status != 0 || result.count != run->len) return 2;
    return result.allocated ? 1 : 0;
}
static uint64_t file_run_count(TSK_FS_FILE *file) {
    const TSK_FS_ATTR *attribute = tsk_fs_file_attr_get(file);
    uint64_t count = 0;
    if (attribute && (attribute->flags & TSK_FS_ATTR_NONRES) && attribute->nrd.initsize >= 0) {
        for (const TSK_FS_ATTR_RUN *p = attribute->nrd.run; p; p = p->next) {
            if (++count > MAX_RUNS) { fail(22); _exit(22); }
        }
    }
    return count;
}
/* Count address-only allocation work without performing it. Saturation denotes
 * a file that cannot fit a fresh child; it must still advance as unchecked. */
static uint64_t file_allocation_work(TSK_FS_FILE *file) {
    const TSK_FS_ATTR *attribute = tsk_fs_file_attr_get(file);
    uint64_t work = 0;
    if (attribute && (attribute->flags & TSK_FS_ATTR_NONRES) && attribute->nrd.initsize >= 0) {
        for (const TSK_FS_ATTR_RUN *p = attribute->nrd.run; p; p = p->next) {
            if (p->flags != TSK_FS_ATTR_RUN_FLAG_NONE || p->len == 0 || p->addr >= file->fs_info->block_count || p->len > file->fs_info->block_count - p->addr) continue;
            if (p->len > MAX_ALLOCATION_CHECKS - work) return MAX_ALLOCATION_CHECKS + 1U;
            work += p->len;
        }
    }
    return work;
}
static int emit_file(TSK_FS_FILE *file) {
    if (failure) return 1;
    if (!file || !file->meta || file->meta->type != TSK_FS_META_TYPE_REG) return 0;
    if (++files > MAX_FILES) { fail(20); return 1; }
    if (file->meta->size < 0) { fail(21); return 1; }
    const TSK_FS_ATTR *attribute = tsk_fs_file_attr_get(file);
    uint64_t count = 0, initialized = 0;
    uint32_t flags = 0, skip = 0;
    const TSK_FS_ATTR_RUN *run = NULL;
    if (attribute) {
        flags = attribute->flags;
        if ((flags & TSK_FS_ATTR_NONRES) && attribute->nrd.initsize >= 0) {
            run = attribute->nrd.run;
            initialized = (uint64_t)attribute->nrd.initsize;
            skip = attribute->nrd.skiplen;
            for (const TSK_FS_ATTR_RUN *p = run; p; p = p->next) {
                if (++count > MAX_RUNS) { fail(22); return 1; }
            }
        }
    }
    if (failure) return 1;
    printf("FILE %" PRIu64 " %" PRIu64 " %u %u %u %" PRIu64 " %" PRIu64 " %u\n",
        (uint64_t)file->meta->addr, (uint64_t)file->meta->size, (unsigned)file->meta->flags,
        file->name ? (unsigned)file->name->flags : 0, flags, count, initialized, skip);
    for (const TSK_FS_ATTR_RUN *p = run; p; p = p->next) {
        /* Allocation checks can request metadata reads; READ records may occur
         * between FILE/RUN records. The parent handles them in every state. */
        int allocated = allocation_consistent(file->fs_info, p);
        if (failure) return 1;
        printf("RUN %" PRIu64 " %" PRIu64 " %" PRIu64 " %u %d\n",
            (uint64_t)p->offset, (uint64_t)p->addr, (uint64_t)p->len, (unsigned)p->flags, allocated);
    }
    printf("END_FILE\n");
    fflush(stdout);
    return 0;
}
int main(int argc, char **argv) {
    uint64_t image_size, filesystem_start;
    if (argc != 9 || !fh_parse_u64(argv[1], &image_size) || !fh_parse_u64(argv[2], &filesystem_start) || image_size == 0 || image_size > INT64_MAX || filesystem_start >= image_size || filesystem_start % 512 != 0) return 2;
    uint64_t directory_inode = 0, entry_start = 0, entry_limit = 0, run_budget = 0;
    if ((strcmp(argv[4], "batch") || (strcmp(argv[5], "root") && !fh_parse_u64(argv[5], &directory_inode)) || !fh_parse_u64(argv[6], &entry_start) || !fh_parse_u64(argv[7], &entry_limit) || !fh_parse_u64(argv[8], &run_budget) || entry_limit == 0 || entry_limit > MAX_FILES || entry_start > 1000000 || run_budget == 0 || run_budget > MAX_RUNS)) return 2;
    TSK_FS_TYPE_ENUM type;
    if (strcmp(argv[3], "fat32") == 0) type = TSK_FS_TYPE_FAT32;
    else if (strcmp(argv[3], "exfat") == 0) type = TSK_FS_TYPE_EXFAT;
    else return 2;
    if (!fh_apply_limits()) return 3;
    printf("HELLO %s %s\n", TSK_VERSION_STR, tsk_version_get_str()); fflush(stdout);
    if (strcmp(TSK_VERSION_STR, "4.14.0") || strcmp(TSK_VERSION_STR, tsk_version_get_str())) { fail(1); return 4; }
    ControlledImage *controlled = calloc(1, sizeof(*controlled));
    if (!controlled) { fail(2); return 5; }
    TSK_IMG_INFO *image = tsk_img_open_external(controlled, (TSK_OFF_T)image_size, 512, controlled_read, controlled_close, controlled_stat);
    if (!image) { free(controlled); fail(3); return 6; }
    TSK_FS_INFO *fs = tsk_fs_open_img(image, (TSK_OFF_T)filesystem_start, type);
    if (!fs) { fail(4); tsk_img_close(image); return 7; }
    if (fs->ftype != type || fs->block_pre_size || fs->block_post_size || fs->block_size == 0 || fs->block_size > MAX_READ) { fail(5); tsk_fs_close(fs); tsk_img_close(image); return 8; }
    printf("GEOMETRY %u %u %" PRIu64 " %" PRIu64 "\n", (unsigned)fs->ftype, fs->block_size, (uint64_t)fs->block_count, (uint64_t)fs->root_inum); fflush(stdout);
    uint64_t next_entry = entry_start, total_entries = 0;
    {
        if (strcmp(argv[5], "root") == 0) directory_inode = fs->root_inum;
        if (directory_inode < fs->first_inum || directory_inode > fs->last_inum) { fail(30); _exit(30); }
        TSK_FS_DIR *directory = tsk_fs_dir_open_meta(fs, (TSK_INUM_T)directory_inode);
        if (!directory) { fail(31); _exit(31); }
        total_entries = tsk_fs_dir_getsize(directory);
        if (total_entries > 1000000 || entry_start > total_entries) { fail(32); _exit(32); }
        printf("DIRECTORY %" PRIu64 " %" PRIu64 " %" PRIu64 "\n", directory_inode, total_entries, entry_start); fflush(stdout);
        uint64_t runs_emitted = 0;
        unsigned stop_reason = 0;
        while (next_entry < total_entries && next_entry - entry_start < entry_limit) {
            const TSK_FS_NAME *name = tsk_fs_dir_get_name(directory, (size_t)next_entry);
            if (!name) { fail(33); _exit(33); }
            int ignored = name->name && (!strcmp(name->name, ".") || !strcmp(name->name, "..") || !strcmp(name->name, ".DS_Store"));
            int virtual_entry = name->type == TSK_FS_NAME_TYPE_VIRT || name->type == TSK_FS_NAME_TYPE_VIRT_DIR;
            TSK_FS_FILE *file = NULL;
            uint64_t next_runs = 0;
            if (!ignored && !virtual_entry) {
                file = tsk_fs_dir_get(directory, (size_t)next_entry);
                if (!file) { fail(34); _exit(34); }
                if (file->meta && file->meta->type == TSK_FS_META_TYPE_REG) {
                    next_runs = file_run_count(file);
                    if (next_runs > run_budget - runs_emitted) {
                        tsk_fs_file_close(file); stop_reason = 2; break;
                    }
                    uint64_t work = file_allocation_work(file);
                    if (work <= MAX_ALLOCATION_CHECKS && work > MAX_ALLOCATION_CHECKS - allocation_checks) {
                        tsk_fs_file_close(file); stop_reason = 3; break;
                    }
                }
            }
            printf("ENTRY %" PRIu64 "\n", next_entry); fflush(stdout);
            if (ignored) {
                printf("IGNORED\n");
            } else if (name->type == TSK_FS_NAME_TYPE_VIRT || name->type == TSK_FS_NAME_TYPE_VIRT_DIR) {
                printf("EXCLUDED %u\n", (unsigned)(name->type == TSK_FS_NAME_TYPE_VIRT_DIR));
            } else {
                if (!file->meta) {
                    printf("DEFERRED %" PRIu64 " 0\n", (uint64_t)name->meta_addr);
                } else if (file->meta->type == TSK_FS_META_TYPE_DIR) {
                    if ((name->flags & TSK_FS_NAME_FLAG_UNALLOC) || !(name->flags & TSK_FS_NAME_FLAG_ALLOC) || !(file->meta->flags & TSK_FS_META_FLAG_ALLOC) || (file->meta->flags & TSK_FS_META_FLAG_UNALLOC)) {
                        printf("DEFERRED %" PRIu64 " 1\n", (uint64_t)name->meta_addr);
                    } else {
                        printf("CHILD %" PRIu64 " %u %u\n", (uint64_t)file->meta->addr, (unsigned)name->flags, (unsigned)file->meta->flags);
                    }
                } else if (file->meta->type == TSK_FS_META_TYPE_REG) {
                    if (emit_file(file) != 0 || failure) { tsk_fs_file_close(file); fail(35); _exit(35); }
                    runs_emitted += next_runs;
                } else {
                    printf("DEFERRED %" PRIu64 " 2\n", (uint64_t)name->meta_addr);
                }
                tsk_fs_file_close(file);
            }
            next_entry++;
            printf("ENTRY_END %" PRIu64 "\n", next_entry); fflush(stdout);
        }
        if (!stop_reason && next_entry != total_entries) stop_reason = 1;
        printf("BATCH_END %" PRIu64 " %u %" PRIu64 " %u\n", next_entry, (unsigned)(next_entry == total_entries), allocation_checks, stop_reason); fflush(stdout);
        tsk_fs_dir_close(directory);
    }
    uint64_t reads = controlled->reads;
    tsk_fs_close(fs); tsk_img_close(image);
    if (failure) return 9;
    uint64_t max_rss;
    if (fh_max_rss_bytes(&max_rss)) printf("METRICS %" PRIu64 "\n", max_rss);
    printf("DONE %" PRIu64 " %" PRIu64 "\n", files, reads); fflush(stdout);
    return 0;
}
