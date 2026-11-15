/* Optional FFmpeg 9.0.2 strict validator. Takes only artifact length; no path,
 * URL or image descriptor. All seekable input bytes are served by the parent.
 * Packet identity/timing and complete decode are separate output observations.
 */
#include "limits.h"
#include <inttypes.h>
#include <stdarg.h>
#include <string.h>
#include <libavformat/avformat.h>
#include <libavcodec/avcodec.h>
#include <libavutil/avutil.h>
#include <libavutil/sha.h>
#include <libavutil/mem.h>

#define TRACK_CAP 8
#define READ_CAP 262144U
#define RECORD_CAP 1000000U
#define BUFFER_BYTES 65536U
#define MAX_WIDTH 4096
#define MAX_HEIGHT 2160
#define MAX_AUDIO_SAMPLES 8192
#define STRICT_ERRORS (AV_EF_CRCCHECK | AV_EF_BITSTREAM | AV_EF_BUFFER | AV_EF_EXPLODE | AV_EF_CAREFUL)

static uint64_t warning_count = 0, error_count = 0, packet_count = 0, frame_count = 0;
static int io_failed = 0;
typedef struct { uint64_t size, cursor, reads; } ControlledInput;
typedef struct { AVCodecContext *context; uint64_t packets, frames; } TrackDecoder;

static void log_observation(void *context, int level, const char *format, va_list arguments) {
    (void)context; (void)format; (void)arguments;
    if (level <= AV_LOG_ERROR) error_count++;
    else if (level <= AV_LOG_WARNING) warning_count++;
}
static void fail(unsigned stage, int code) {
    printf("ERROR %u %d %" PRIu64 " %" PRIu64 "\n", stage, code, warning_count, error_count);
    fflush(stdout);
}
static int read_packet(void *opaque, uint8_t *buffer, int requested) {
    ControlledInput *input = opaque;
    if (io_failed || requested <= 0 || input->cursor > input->size || ++input->reads > READ_CAP) { io_failed = 1; return AVERROR(EIO); }
    if (input->cursor == input->size) return AVERROR_EOF;
    size_t length = (size_t)((input->size - input->cursor < (uint64_t)requested) ? input->size - input->cursor : (uint64_t)requested);
    if (length > BUFFER_BYTES) length = BUFFER_BYTES;
    printf("READ %" PRIu64 " %zu\n", input->cursor, length); fflush(stdout);
    if (!fh_receive_bytes(buffer, length)) { io_failed = 1; return AVERROR(EIO); }
    input->cursor += length;
    return (int)length;
}
static int64_t seek_packet(void *opaque, int64_t offset, int whence) {
    ControlledInput *input = opaque;
    whence &= ~AVSEEK_FORCE;
    if (whence == AVSEEK_SIZE) return (int64_t)input->size;
    int64_t base;
    if (whence == SEEK_SET) base = 0;
    else if (whence == SEEK_CUR) base = (int64_t)input->cursor;
    else if (whence == SEEK_END) base = (int64_t)input->size;
    else return AVERROR(EINVAL);
    if (offset < -base || offset > INT64_MAX - base) return AVERROR(EINVAL);
    int64_t target = base + offset;
    if (target < 0 || (uint64_t)target > input->size) return AVERROR(EINVAL);
    input->cursor = (uint64_t)target;
    return target;
}
static int deny_open(AVFormatContext *format, AVIOContext **io, const char *url, int flags, AVDictionary **options) {
    (void)format; (void)io; (void)url; (void)flags; (void)options;
    return AVERROR(EPERM);
}
static int sha256_hex(const uint8_t *bytes, size_t length, char output[65]) {
    struct AVSHA *sha = av_sha_alloc();
    if (!sha || av_sha_init(sha, 256) != 0) { av_free(sha); return 0; }
    if (length) av_sha_update(sha, bytes, length);
    uint8_t digest[32]; av_sha_final(sha, digest); av_free(sha);
    static const char alphabet[] = "0123456789abcdef";
    for (size_t i = 0; i < 32; i++) { output[2*i] = alphabet[digest[i] >> 4]; output[2*i+1] = alphabet[digest[i] & 15]; }
    output[64] = 0; return 1;
}
static void timestamp(int64_t value, char output[32]) {
    if (value == AV_NOPTS_VALUE) strcpy(output, "NOPTS");
    else snprintf(output, 32, "%" PRId64, value);
}
static int receive_frames(TrackDecoder *decoder, unsigned stream_index, AVFrame *frame, int draining) {
    int status;
    while ((status = avcodec_receive_frame(decoder->context, frame)) >= 0) {
        if (++frame_count > RECORD_CAP || frame->flags & AV_FRAME_FLAG_CORRUPT || frame->decode_error_flags != 0 || frame->width < 0 || frame->width > MAX_WIDTH || frame->height < 0 || frame->height > MAX_HEIGHT || frame->nb_samples < 0 || frame->nb_samples > MAX_AUDIO_SAMPLES || frame->ch_layout.nb_channels > 8) { av_frame_unref(frame); return AVERROR_INVALIDDATA; }
        char pts[32]; timestamp(frame->pts, pts);
        printf("FRAME %u %" PRIu64 " %s %" PRId64 " %d %u %d %d %d\n", stream_index, decoder->frames++, pts, frame->duration, frame->nb_samples, (unsigned)frame->flags, frame->decode_error_flags, frame->width, frame->height);
        av_frame_unref(frame);
    }
    if (status == AVERROR_EOF) return status;
    if (status == AVERROR(EAGAIN) && !draining) return 0;
    return status;
}
int main(int argc, char **argv) {
    ControlledInput input = {0,0,0};
    if (argc != 2 || !fh_parse_u64(argv[1], &input.size) || input.size == 0 || input.size > INT64_MAX) return 2;
    if (!fh_apply_limits()) return 3;
    printf("HELLO FFMPEG %s %u %u %u %u %u %u\n", av_version_info(), LIBAVFORMAT_VERSION_INT, avformat_version(), LIBAVCODEC_VERSION_INT, avcodec_version(), LIBAVUTIL_VERSION_INT, avutil_version()); fflush(stdout);
    if (strcmp(av_version_info(), "9.0.2") || LIBAVFORMAT_VERSION_INT != avformat_version() || LIBAVCODEC_VERSION_INT != avcodec_version() || LIBAVUTIL_VERSION_INT != avutil_version()) { fail(1,0); return 4; }
    av_log_set_level(AV_LOG_WARNING); av_log_set_callback(log_observation); av_max_alloc(64 * 1024 * 1024);
    printf("POLICY 1 %u 1\n", (unsigned)STRICT_ERRORS); fflush(stdout);
    int result = 1, status = 0;
    unsigned phase = 2;
    AVFormatContext *format = avformat_alloc_context();
    AVIOContext *io = NULL;
    AVDictionary *options = NULL;
    AVPacket *packet = NULL;
    AVFrame *frame = NULL;
    TrackDecoder decoders[TRACK_CAP] = {{0}};
    uint8_t *buffer = av_mallocz(BUFFER_BYTES + AV_INPUT_BUFFER_PADDING_SIZE);
    if (!format || !buffer) { av_free(buffer); status = AVERROR(ENOMEM); goto done; }
    io = avio_alloc_context(buffer, BUFFER_BYTES, 0, &input, read_packet, NULL, seek_packet);
    if (!io) { av_free(buffer); status = AVERROR(ENOMEM); goto done; }
    format->pb = io; format->flags |= AVFMT_FLAG_CUSTOM_IO;
    format->io_open = deny_open; format->error_recognition = STRICT_ERRORS;
    format->max_streams = TRACK_CAP; format->probesize = 1024 * 1024; format->format_probesize = 1024 * 1024;
    io->seekable = AVIO_SEEKABLE_NORMAL;
    av_dict_set(&options, "ignore_editlist", "1", 0);
    phase = 3;
    status = avformat_open_input(&format, NULL, av_find_input_format("mov"), &options);
    int unused_options = av_dict_count(options);
    av_dict_free(&options);
    if (status < 0 || !format || io_failed) goto done;
    if (unused_options) { status = AVERROR_OPTION_NOT_FOUND; goto done; }
    if (format->nb_streams == 0 || format->nb_streams > TRACK_CAP) { status = AVERROR_INVALIDDATA; goto done; }
    phase = 4;
    for (unsigned i = 0; i < format->nb_streams; i++) {
        AVStream *stream = format->streams[i];
        AVCodecParameters *parameters = stream->codecpar;
        if (!((parameters->codec_type == AVMEDIA_TYPE_VIDEO && (parameters->codec_id == AV_CODEC_ID_H264 || parameters->codec_id == AV_CODEC_ID_HEVC)) || (parameters->codec_type == AVMEDIA_TYPE_AUDIO && parameters->codec_id == AV_CODEC_ID_AAC)) || parameters->extradata_size < 0 || parameters->extradata_size > 1024 * 1024 || stream->time_base.num <= 0 || stream->time_base.den <= 0) { status = AVERROR_INVALIDDATA; goto done; }
        const AVCodec *codec = avcodec_find_decoder(parameters->codec_id);
        if (!codec || !(decoders[i].context = avcodec_alloc_context3(codec))) { status = AVERROR(ENOMEM); goto done; }
        status = avcodec_parameters_to_context(decoders[i].context, parameters);
        if (status < 0) goto done;
        decoders[i].context->thread_count = 1; decoders[i].context->thread_type = 0;
        decoders[i].context->err_recognition = STRICT_ERRORS; decoders[i].context->error_concealment = 0;
        decoders[i].context->max_pixels = (int64_t)MAX_WIDTH * (MAX_HEIGHT + 16);
        decoders[i].context->max_samples = MAX_AUDIO_SAMPLES;
        decoders[i].context->pkt_timebase = stream->time_base;
        status = avcodec_open2(decoders[i].context, codec, NULL);
        if (status < 0) goto done;
        if (decoders[i].context->err_recognition != STRICT_ERRORS || decoders[i].context->error_concealment != 0 || decoders[i].context->thread_count != 1) { status = AVERROR(EINVAL); goto done; }
        char hash[65];
        if (!sha256_hex(parameters->extradata, (size_t)parameters->extradata_size, hash)) { status = AVERROR(ENOMEM); goto done; }
        /* Decoder initialization interprets AAC ASC independently of legacy
         * sample-entry defaults (which may still say stereo for mono AAC). */
        printf("STREAM %u %d %d %u %d %d %d %d %s %d %d %d %d\n", i, stream->id, parameters->codec_id, parameters->codec_tag, parameters->codec_type, stream->time_base.num, stream->time_base.den, parameters->extradata_size, hash, parameters->width, parameters->height, decoders[i].context->sample_rate, decoders[i].context->ch_layout.nb_channels);
    }
    packet = av_packet_alloc(); frame = av_frame_alloc();
    if (!packet || !frame) { status = AVERROR(ENOMEM); goto done; }
    phase = 5;
    while ((status = av_read_frame(format, packet)) >= 0) {
        if (io_failed || packet->stream_index < 0 || (unsigned)packet->stream_index >= format->nb_streams || packet->size <= 0 || packet->flags & AV_PKT_FLAG_CORRUPT || ++packet_count > RECORD_CAP) { status = AVERROR_INVALIDDATA; goto done; }
        unsigned index = (unsigned)packet->stream_index;
        char hash[65], dts[32], pts[32];
        timestamp(packet->dts, dts); timestamp(packet->pts, pts);
        if (!sha256_hex(packet->data, (size_t)packet->size, hash)) { status = AVERROR(ENOMEM); goto done; }
        printf("PACKET %u %" PRIu64 " %" PRId64 " %d %s %s %" PRId64 " %u %s\n", index, decoders[index].packets++, packet->pos, packet->size, dts, pts, packet->duration, (unsigned)packet->flags, hash);
        status = avcodec_send_packet(decoders[index].context, packet);
        if (status == AVERROR(EAGAIN)) { status = receive_frames(&decoders[index], index, frame, 0); if (status >= 0) status = avcodec_send_packet(decoders[index].context, packet); }
        av_packet_unref(packet);
        if (status < 0) goto done;
        status = receive_frames(&decoders[index], index, frame, 0);
        if (status < 0) goto done;
        if (warning_count || error_count) { status = AVERROR_INVALIDDATA; goto done; }
    }
    if (status != AVERROR_EOF || io_failed) goto done;
    phase = 6;
    for (unsigned i = 0; i < format->nb_streams; i++) {
        status = avcodec_send_packet(decoders[i].context, NULL);
        if (status < 0 && status != AVERROR_EOF) goto done;
        status = receive_frames(&decoders[i], i, frame, 1);
        if (status != AVERROR_EOF) goto done;
        if (decoders[i].packets > 0 && decoders[i].frames == 0) { status = AVERROR_INVALIDDATA; goto done; }
    }
    if (warning_count || error_count || packet_count == 0 || frame_count == 0) { status = AVERROR_INVALIDDATA; goto done; }
    result = 0;
done:
    av_packet_free(&packet); av_frame_free(&frame);
    for (unsigned i = 0; i < TRACK_CAP; i++) avcodec_free_context(&decoders[i].context);
    av_dict_free(&options); avformat_close_input(&format);
    if (io) av_freep(&io->buffer);
    avio_context_free(&io);
    if (!result && (warning_count || error_count)) { phase = 7; status = AVERROR_INVALIDDATA; result = 1; }
    if (result) fail(phase, status);
    else {
        uint64_t rss = 0; fh_max_rss_bytes(&rss);
        printf("SUMMARY %" PRIu64 " %" PRIu64 " %" PRIu64 " %" PRIu64 " %" PRIu64 "\n", packet_count, frame_count, warning_count, error_count, rss);
        printf("DONE\n"); fflush(stdout);
    }
    return result;
}
