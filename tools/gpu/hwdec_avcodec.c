#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <fcntl.h>
#include <pthread.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>

#include <libavcodec/avcodec.h>
#include <libavutil/error.h>
#include <libavutil/frame.h>
#include <libavutil/pixfmt.h>

#define CODEC_AV1  1u
#define CODEC_VP9  2u
#define CODEC_H264 3u
#define CODEC_HEVC 4u

#define MEDIA_SOCKET_PATH "/run/bareboot-gpu/socket/bareboot-media-0"
#define YUV_SHM_SLOTS 16u
#define YUV_SLOT_BYTES (16u * 1024u * 1024u)
#define MAX_QUEUE 32u
#define MAX_DECODERS 32u

struct QueuedFrame {
    uint32_t slot;
    uint32_t width;
    uint32_t height;
    uint32_t y_off;
    uint32_t y_stride;
    uint32_t u_off;
    uint32_t v_off;
    uint32_t c_stride;
    int64_t pts_us;
};

struct BarebootDec {
    AVCodecContext *avctx;
    uint32_t codec_id;
    int media_fd;
    int shm_fd;
    uint8_t *shm_ptr;
    size_t shm_size;
    int nal_len_size;
    bool eos_reached;
    bool opened;
    struct QueuedFrame queue[MAX_QUEUE];
    uint32_t q_head;
    uint32_t q_tail;
    uint32_t q_count;
};

static pthread_mutex_t g_dec_lock = PTHREAD_MUTEX_INITIALIZER;
static struct BarebootDec *g_decs[MAX_DECODERS];

static uint32_t av_codec_id_to_bareboot(enum AVCodecID id) {
    switch (id) {
    case AV_CODEC_ID_AV1:  return CODEC_AV1;
    case AV_CODEC_ID_VP9:  return CODEC_VP9;
    case AV_CODEC_ID_H264: return CODEC_H264;
    case AV_CODEC_ID_HEVC: return CODEC_HEVC;
    default: return 0;
    }
}

static struct BarebootDec *find_dec(const AVCodecContext *avctx) {
    pthread_mutex_lock(&g_dec_lock);
    struct BarebootDec *found = NULL;
    for (uint32_t i = 0; i < MAX_DECODERS; i++) {
        if (g_decs[i] && g_decs[i]->avctx == avctx) {
            found = g_decs[i];
            break;
        }
    }
    pthread_mutex_unlock(&g_dec_lock);
    return found;
}

static struct BarebootDec *remove_dec(const AVCodecContext *avctx) {
    pthread_mutex_lock(&g_dec_lock);
    struct BarebootDec *found = NULL;
    for (uint32_t i = 0; i < MAX_DECODERS; i++) {
        if (g_decs[i] && g_decs[i]->avctx == avctx) {
            found = g_decs[i];
            g_decs[i] = NULL;
            break;
        }
    }
    pthread_mutex_unlock(&g_dec_lock);
    return found;
}

static void close_dec(struct BarebootDec *dec) {
    if (!dec) return;
    if (dec->shm_ptr && dec->shm_ptr != MAP_FAILED) {
        munmap(dec->shm_ptr, dec->shm_size);
        dec->shm_ptr = NULL;
    }
    if (dec->shm_fd >= 0) {
        close(dec->shm_fd);
        dec->shm_fd = -1;
    }
    if (dec->media_fd >= 0) {
        close(dec->media_fd);
        dec->media_fd = -1;
    }
}

static bool send_all(int fd, const void *buf, size_t len) {
    const uint8_t *p = buf;
    size_t sent = 0;
    while (sent < len) {
        ssize_t n;
        do {
            n = write(fd, p + sent, len - sent);
        } while (n < 0 && errno == EINTR);
        if (n <= 0) return false;
        sent += (size_t)n;
    }
    return true;
}

static bool recv_pkt(int fd, uint32_t *kind, uint8_t **out_buf, uint32_t *out_len,
                     int *fds, size_t *fd_count) {
    uint8_t hdr[8];
    union {
        struct cmsghdr align;
        unsigned char data[CMSG_SPACE(8 * sizeof(int))];
    } control;
    *fd_count = 0;
    size_t got = 0;
    while (got < 8) {
        struct iovec iov = {.iov_base = hdr + got, .iov_len = 8 - got};
        struct msghdr msg = {.msg_iov = &iov, .msg_iovlen = 1};
        if (got == 0) {
            msg.msg_control = &control;
            msg.msg_controllen = sizeof(control);
        }
        ssize_t n;
        do {
            n = recvmsg(fd, &msg, MSG_CMSG_CLOEXEC);
        } while (n < 0 && errno == EINTR);
        if (n <= 0) return false;
        if (got == 0) {
            for (struct cmsghdr *h = CMSG_FIRSTHDR(&msg); h; h = CMSG_NXTHDR(&msg, h)) {
                if (h->cmsg_level == SOL_SOCKET && h->cmsg_type == SCM_RIGHTS) {
                    size_t cnt = (h->cmsg_len - CMSG_LEN(0)) / sizeof(int);
                    for (size_t i = 0; i < cnt && *fd_count < 8; i++) {
                        int rfd;
                        memcpy(&rfd, CMSG_DATA(h) + i * sizeof(int), sizeof(int));
                        fds[(*fd_count)++] = rfd;
                    }
                }
            }
        }
        got += (size_t)n;
    }
    memcpy(kind, hdr, 4);
    memcpy(out_len, hdr + 4, 4);
    *out_buf = NULL;
    if (*out_len > 0) {
        *out_buf = malloc(*out_len);
        if (!*out_buf) return false;
        size_t pgot = 0;
        while (pgot < *out_len) {
            ssize_t n;
            do {
                n = read(fd, *out_buf + pgot, *out_len - pgot);
            } while (n < 0 && errno == EINTR);
            if (n <= 0) {
                free(*out_buf);
                *out_buf = NULL;
                return false;
            }
            pgot += (size_t)n;
        }
    }
    return true;
}

static bool parse_avcc(const uint8_t *ex, size_t len, int *nal_len_size,
                       uint8_t **csd0, uint32_t *csd0_len,
                       uint8_t **csd1, uint32_t *csd1_len) {
    if (len < 7 || ex[0] != 1) return false;
    *nal_len_size = (ex[4] & 3) + 1;
    size_t pos = 5;
    uint8_t num_sps = ex[pos++] & 0x1f;
    size_t s0 = 0;
    uint8_t *b0 = malloc(len + 64);
    if (!b0) return false;
    for (uint8_t i = 0; i < num_sps && pos + 2 <= len; i++) {
        uint16_t n = (uint16_t)((ex[pos] << 8) | ex[pos + 1]);
        pos += 2;
        if (pos + n > len) { free(b0); return false; }
        b0[s0++] = 0; b0[s0++] = 0; b0[s0++] = 0; b0[s0++] = 1;
        memcpy(b0 + s0, ex + pos, n);
        s0 += n;
        pos += n;
    }
    size_t s1 = 0;
    uint8_t *b1 = malloc(len + 64);
    if (!b1) { free(b0); return false; }
    if (pos < len) {
        uint8_t num_pps = ex[pos++];
        for (uint8_t i = 0; i < num_pps && pos + 2 <= len; i++) {
            uint16_t n = (uint16_t)((ex[pos] << 8) | ex[pos + 1]);
            pos += 2;
            if (pos + n > len) { free(b0); free(b1); return false; }
            b1[s1++] = 0; b1[s1++] = 0; b1[s1++] = 0; b1[s1++] = 1;
            memcpy(b1 + s1, ex + pos, n);
            s1 += n;
            pos += n;
        }
    }
    *csd0 = b0; *csd0_len = (uint32_t)s0;
    *csd1 = b1; *csd1_len = (uint32_t)s1;
    return true;
}

static bool parse_hvcc(const uint8_t *ex, size_t len, int *nal_len_size,
                       uint8_t **csd0, uint32_t *csd0_len) {
    if (len < 23 || ex[0] != 1) return false;
    *nal_len_size = (ex[21] & 3) + 1;
    uint8_t num_arrays = ex[22];
    size_t pos = 23;
    uint8_t *b0 = malloc(len + 128);
    if (!b0) return false;
    size_t s0 = 0;
    for (uint8_t a = 0; a < num_arrays && pos + 3 <= len; a++) {
        pos += 1;
        uint16_t num_nalus = (uint16_t)((ex[pos] << 8) | ex[pos + 1]);
        pos += 2;
        for (uint16_t i = 0; i < num_nalus && pos + 2 <= len; i++) {
            uint16_t n = (uint16_t)((ex[pos] << 8) | ex[pos + 1]);
            pos += 2;
            if (pos + n > len) { free(b0); return false; }
            b0[s0++] = 0; b0[s0++] = 0; b0[s0++] = 0; b0[s0++] = 1;
            memcpy(b0 + s0, ex + pos, n);
            s0 += n;
            pos += n;
        }
    }
    *csd0 = b0; *csd0_len = (uint32_t)s0;
    return true;
}

static uint8_t *convert_len_prefixed_to_annexb(const uint8_t *src, size_t len, int nls, size_t *out_len) {
    uint8_t *dst = malloc(len + 256);
    if (!dst) return NULL;
    size_t pos = 0, out = 0, cap = len + 256;
    while (pos + (size_t)nls <= len) {
        uint32_t n = 0;
        for (int i = 0; i < nls; i++) n = (n << 8) | src[pos++];
        if (pos + n > len) break;
        if (out + 4 + n > cap) {
            cap = (out + 4 + n) * 2;
            uint8_t *nb = realloc(dst, cap);
            if (!nb) { free(dst); return NULL; }
            dst = nb;
        }
        dst[out++] = 0; dst[out++] = 0; dst[out++] = 0; dst[out++] = 1;
        memcpy(dst + out, src + pos, n);
        out += n;
        pos += n;
    }
    *out_len = out;
    return dst;
}

int avcodec_open2(AVCodecContext *avctx, const AVCodec *codec, AVDictionary **options) {
    static int (*real_fn)(AVCodecContext *, const AVCodec *, AVDictionary **);
    if (!real_fn) real_fn = dlsym(RTLD_NEXT, "avcodec_open2");

    const AVCodec *c = codec ? codec : (avctx ? avctx->codec : NULL);
    uint32_t cid = (avctx && c && c->type == AVMEDIA_TYPE_VIDEO && av_codec_is_decoder(c) &&
                    !avctx->hw_device_ctx && !avctx->hw_frames_ctx &&
                    (!c->name || !strstr(c->name, "v4l2m2m")))
                   ? av_codec_id_to_bareboot(c->id)
                   : 0;

    if (cid && access(MEDIA_SOCKET_PATH, F_OK) == 0) {
        int fd = socket(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0);
        if (fd >= 0) {
            struct sockaddr_un addr;
            memset(&addr, 0, sizeof(addr));
            addr.sun_family = AF_UNIX;
            strncpy(addr.sun_path, MEDIA_SOCKET_PATH, sizeof(addr.sun_path) - 1);
            if (connect(fd, (struct sockaddr *)&addr, sizeof(addr)) == 0) {
                uint32_t w = avctx->width >= 64 ? (uint32_t)avctx->width : 1920u;
                uint32_t h = avctx->height >= 64 ? (uint32_t)avctx->height : 1080u;
                int nal_len_size = 0;
                uint8_t *csd0 = NULL, *csd1 = NULL;
                uint32_t csd0_len = 0, csd1_len = 0;
                if (avctx->extradata && avctx->extradata_size > 0) {
                    if (cid == CODEC_H264 && avctx->extradata[0] == 1) {
                        parse_avcc(avctx->extradata, (size_t)avctx->extradata_size, &nal_len_size,
                                   &csd0, &csd0_len, &csd1, &csd1_len);
                    } else if (cid == CODEC_HEVC && avctx->extradata[0] == 1) {
                        parse_hvcc(avctx->extradata, (size_t)avctx->extradata_size, &nal_len_size,
                                   &csd0, &csd0_len);
                    } else if (cid == CODEC_H264 || cid == CODEC_HEVC) {
                        csd0 = malloc((size_t)avctx->extradata_size);
                        if (csd0) {
                            memcpy(csd0, avctx->extradata, (size_t)avctx->extradata_size);
                            csd0_len = (uint32_t)avctx->extradata_size;
                        }
                    }
                }
                uint32_t payload_len = 20u + csd0_len + csd1_len;
                uint8_t *pkt = malloc(8u + payload_len);
                if (pkt) {
                    uint32_t kind = 10u;
                    memcpy(pkt + 0, &kind, 4);
                    memcpy(pkt + 4, &payload_len, 4);
                    memcpy(pkt + 8, &cid, 4);
                    memcpy(pkt + 12, &w, 4);
                    memcpy(pkt + 16, &h, 4);
                    memcpy(pkt + 20, &csd0_len, 4);
                    memcpy(pkt + 24, &csd1_len, 4);
                    if (csd0_len) memcpy(pkt + 28, csd0, csd0_len);
                    if (csd1_len) memcpy(pkt + 28 + csd0_len, csd1, csd1_len);
                    bool sent = send_all(fd, pkt, 8u + payload_len);
                    free(pkt);
                    free(csd0);
                    free(csd1);
                    if (sent) {
                        uint32_t rkind = 0, rlen = 0;
                        uint8_t *rbuf = NULL;
                        int rfds[8];
                        size_t rfd_cnt = 0;
                        if (recv_pkt(fd, &rkind, &rbuf, &rlen, rfds, &rfd_cnt) &&
                            rkind == 1 && rlen >= 4 && rfd_cnt == 1) {
                            uint32_t status = 1;
                            memcpy(&status, rbuf, 4);
                            if (status == 0) {
                                size_t shm_size = (size_t)YUV_SHM_SLOTS * (size_t)YUV_SLOT_BYTES;
                                uint8_t *shm_ptr = mmap(NULL, shm_size, PROT_READ, MAP_SHARED, rfds[0], 0);
                                if (shm_ptr != MAP_FAILED) {
                                    struct BarebootDec *dec = calloc(1, sizeof(*dec));
                                    if (dec) {
                                        dec->avctx = avctx;
                                        dec->codec_id = cid;
                                        dec->media_fd = fd;
                                        dec->shm_fd = rfds[0];
                                        dec->shm_ptr = shm_ptr;
                                        dec->shm_size = shm_size;
                                        dec->nal_len_size = nal_len_size;
                                        dec->opened = true;
                                        pthread_mutex_lock(&g_dec_lock);
                                        bool stored = false;
                                        for (uint32_t i = 0; i < MAX_DECODERS; i++) {
                                            if (!g_decs[i]) {
                                                g_decs[i] = dec;
                                                stored = true;
                                                break;
                                            }
                                        }
                                        pthread_mutex_unlock(&g_dec_lock);
                                        if (stored) {
                                            avctx->codec = c;
                                            avctx->codec_id = c->id;
                                            avctx->codec_type = AVMEDIA_TYPE_VIDEO;
                                            avctx->pix_fmt = AV_PIX_FMT_YUV420P;
                                            fprintf(stderr, "[bareboot-hwdec] active: %.*s (%ux%u)\n",
                                                    (int)(rlen > 4 ? rlen - 4 : 0),
                                                    rlen > 4 ? (const char *)(rbuf + 4) : "hw", w, h);
                                            free(rbuf);
                                            return 0;
                                        }
                                        close_dec(dec);
                                        free(dec);
                                        free(rbuf);
                                        return real_fn ? real_fn(avctx, codec, options) : AVERROR(ENOSYS);
                                    }
                                    munmap(shm_ptr, shm_size);
                                }
                            }
                        }
                        free(rbuf);
                        for (size_t i = 0; i < rfd_cnt; i++) close(rfds[i]);
                    }
                } else {
                    free(csd0);
                    free(csd1);
                }
            }
            close(fd);
        }
    }

    return real_fn ? real_fn(avctx, codec, options) : AVERROR(ENOSYS);
}

int avcodec_send_packet(AVCodecContext *avctx, const AVPacket *avpkt) {
    struct BarebootDec *dec = find_dec(avctx);
    if (!dec) {
        static int (*real_fn)(AVCodecContext *, const AVPacket *);
        if (!real_fn) real_fn = dlsym(RTLD_NEXT, "avcodec_send_packet");
        return real_fn ? real_fn(avctx, avpkt) : AVERROR(ENOSYS);
    }
    if (!dec->opened || dec->media_fd < 0) return AVERROR(EINVAL);
    if (dec->q_count >= MAX_QUEUE - 4u) return AVERROR(EAGAIN);

    int64_t pts = 0;
    uint32_t flags = 0;
    const uint8_t *data = NULL;
    size_t data_len = 0;
    uint8_t *converted = NULL;

    if (!avpkt || (!avpkt->data && avpkt->size == 0)) {
        flags = 1u;
    } else {
        pts = avpkt->pts != AV_NOPTS_VALUE ? avpkt->pts : (avpkt->dts != AV_NOPTS_VALUE ? avpkt->dts : 0);
        if (dec->nal_len_size > 0 && avpkt->size > 0) {
            converted = convert_len_prefixed_to_annexb(avpkt->data, (size_t)avpkt->size,
                                                       dec->nal_len_size, &data_len);
            if (!converted) return AVERROR(ENOMEM);
            data = converted;
        } else {
            data = avpkt->data;
            data_len = (size_t)avpkt->size;
        }
    }

    uint32_t kind = 11u;
    uint32_t payload_len = (uint32_t)(12u + data_len);
    uint8_t hdr[20];
    memcpy(hdr + 0, &kind, 4);
    memcpy(hdr + 4, &payload_len, 4);
    memcpy(hdr + 8, &pts, 8);
    memcpy(hdr + 16, &flags, 4);
    bool ok = send_all(dec->media_fd, hdr, sizeof(hdr));
    if (ok && data_len > 0) ok = send_all(dec->media_fd, data, data_len);
    free(converted);
    if (!ok) return AVERROR(EIO);

    for (;;) {
        uint32_t rkind = 0, rlen = 0;
        uint8_t *rbuf = NULL;
        int rfds[8];
        size_t rfd_cnt = 0;
        if (!recv_pkt(dec->media_fd, &rkind, &rbuf, &rlen, rfds, &rfd_cnt)) {
            return AVERROR(EIO);
        }
        for (size_t i = 0; i < rfd_cnt; i++) close(rfds[i]);
        if (rkind == 12u && rlen == 40u) {
            if (dec->q_count < MAX_QUEUE) {
                struct QueuedFrame *q = &dec->queue[dec->q_tail];
                memcpy(&q->slot, rbuf + 0, 4);
                memcpy(&q->width, rbuf + 4, 4);
                memcpy(&q->height, rbuf + 8, 4);
                memcpy(&q->y_off, rbuf + 12, 4);
                memcpy(&q->y_stride, rbuf + 16, 4);
                memcpy(&q->u_off, rbuf + 20, 4);
                memcpy(&q->v_off, rbuf + 24, 4);
                memcpy(&q->c_stride, rbuf + 28, 4);
                memcpy(&q->pts_us, rbuf + 32, 8);
                dec->q_tail = (dec->q_tail + 1u) % MAX_QUEUE;
                dec->q_count++;
            }
            free(rbuf);
        } else if (rkind == 3u && rlen >= 8u) {
            uint32_t eos = 0;
            memcpy(&eos, rbuf + 4, 4);
            if (eos) dec->eos_reached = true;
            free(rbuf);
            break;
        } else {
            free(rbuf);
            break;
        }
    }
    return 0;
}

int avcodec_receive_frame(AVCodecContext *avctx, AVFrame *frame) {
    struct BarebootDec *dec = find_dec(avctx);
    if (!dec) {
        static int (*real_fn)(AVCodecContext *, AVFrame *);
        if (!real_fn) real_fn = dlsym(RTLD_NEXT, "avcodec_receive_frame");
        return real_fn ? real_fn(avctx, frame) : AVERROR(ENOSYS);
    }
    if (dec->q_count == 0) {
        return dec->eos_reached ? AVERROR_EOF : AVERROR(EAGAIN);
    }

    struct QueuedFrame q = dec->queue[dec->q_head];
    dec->q_head = (dec->q_head + 1u) % MAX_QUEUE;
    dec->q_count--;

    av_frame_unref(frame);
    frame->format = AV_PIX_FMT_YUV420P;
    frame->width = (int)q.width;
    frame->height = (int)q.height;
    frame->pts = q.pts_us;
    frame->pkt_dts = q.pts_us;
    frame->colorspace = avctx->colorspace != AVCOL_SPC_UNSPECIFIED ? avctx->colorspace : AVCOL_SPC_BT709;
    frame->color_range = avctx->color_range != AVCOL_RANGE_UNSPECIFIED ? avctx->color_range : AVCOL_RANGE_MPEG;

    avctx->width = (int)q.width;
    avctx->height = (int)q.height;
    avctx->coded_width = (int)q.width;
    avctx->coded_height = (int)q.height;
    avctx->pix_fmt = AV_PIX_FMT_YUV420P;

    int ret = (avctx->get_buffer2 && avctx->get_buffer2 != avcodec_default_get_buffer2)
                  ? avctx->get_buffer2(avctx, frame, 0)
                  : av_frame_get_buffer(frame, 32);
    if (ret < 0) ret = av_frame_get_buffer(frame, 32);
    if (ret < 0) return ret;

    const uint8_t *base = dec->shm_ptr + (size_t)(q.slot % YUV_SHM_SLOTS) * YUV_SLOT_BYTES;
    const uint8_t *sy = base + q.y_off;
    const uint8_t *su = base + q.u_off;
    const uint8_t *sv = base + q.v_off;
    uint32_t cw = (q.width + 1u) >> 1;
    uint32_t ch = (q.height + 1u) >> 1;

    for (uint32_t r = 0; r < q.height; r++) {
        memcpy(frame->data[0] + (size_t)r * (size_t)frame->linesize[0],
               sy + (size_t)r * q.y_stride, q.width);
    }
    for (uint32_t r = 0; r < ch; r++) {
        memcpy(frame->data[1] + (size_t)r * (size_t)frame->linesize[1],
               su + (size_t)r * q.c_stride, cw);
        memcpy(frame->data[2] + (size_t)r * (size_t)frame->linesize[2],
               sv + (size_t)r * q.c_stride, cw);
    }
    return 0;
}

int avcodec_default_get_buffer2(AVCodecContext *s, AVFrame *frame, int flags) {
    if (find_dec(s)) {
        return av_frame_get_buffer(frame, 32);
    }
    static int (*real_fn)(AVCodecContext *, AVFrame *, int);
    if (!real_fn) real_fn = dlsym(RTLD_NEXT, "avcodec_default_get_buffer2");
    return real_fn ? real_fn(s, frame, flags) : AVERROR(ENOSYS);
}

void avcodec_align_dimensions(AVCodecContext *s, int *width, int *height) {
    if (find_dec(s)) {
        if (width) *width = (*width + 15) & ~15;
        if (height) *height = (*height + 15) & ~15;
        return;
    }
    static void (*real_fn)(AVCodecContext *, int *, int *);
    if (!real_fn) real_fn = dlsym(RTLD_NEXT, "avcodec_align_dimensions");
    if (real_fn) real_fn(s, width, height);
}

void avcodec_flush_buffers(AVCodecContext *avctx) {
    struct BarebootDec *dec = find_dec(avctx);
    if (!dec) {
        static void (*real_fn)(AVCodecContext *);
        if (!real_fn) real_fn = dlsym(RTLD_NEXT, "avcodec_flush_buffers");
        if (real_fn) real_fn(avctx);
        return;
    }
    if (dec->opened && dec->media_fd >= 0) {
        uint32_t hdr[2] = {4u, 0u};
        send_all(dec->media_fd, hdr, sizeof(hdr));
    }
    dec->q_head = dec->q_tail = dec->q_count = 0;
    dec->eos_reached = false;
}

int avcodec_close(AVCodecContext *avctx) {
    struct BarebootDec *dec = find_dec(avctx);
    if (dec) {
        close_dec(dec);
        dec->opened = false;
        return 0;
    }
    static int (*real_fn)(AVCodecContext *);
    if (!real_fn) real_fn = dlsym(RTLD_NEXT, "avcodec_close");
    return real_fn ? real_fn(avctx) : 0;
}

void avcodec_free_context(AVCodecContext **avctx_ptr) {
    if (!avctx_ptr || !*avctx_ptr) return;
    struct BarebootDec *dec = remove_dec(*avctx_ptr);
    if (dec) {
        close_dec(dec);
        (*avctx_ptr)->codec = NULL;
        free(dec);
    }
    static void (*real_fn)(AVCodecContext **);
    if (!real_fn) real_fn = dlsym(RTLD_NEXT, "avcodec_free_context");
    if (real_fn) real_fn(avctx_ptr);
}
