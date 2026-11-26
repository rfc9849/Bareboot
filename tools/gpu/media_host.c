#include <EGL/egl.h>
#include <EGL/eglext.h>
#include <GLES2/gl2.h>
#include <GLES2/gl2ext.h>
#include <android/hardware_buffer.h>
#include <android/native_window.h>
#include <dlfcn.h>
#include <errno.h>
#include <fcntl.h>
#include <media/NdkImageReader.h>
#include <media/NdkMediaCodec.h>
#include <media/NdkMediaFormat.h>
#include <poll.h>
#include <pthread.h>
#include <signal.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <linux/memfd.h>
#include <sys/mman.h>
#include <sys/prctl.h>
#include <sys/syscall.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/un.h>
#include <unistd.h>

#define MAX_FDS 16
#define MAX_METADATA 4096
#define MAX_SLOTS 16
#define MAX_PACKET_BYTES (8 * 1024 * 1024)
#define YUV_SHM_SLOTS 16u
#define YUV_SLOT_BYTES (16u * 1024u * 1024u)

static int g_ahb_fd = -1;
static pthread_mutex_t g_ahb_lock = PTHREAD_MUTEX_INITIALIZER;

struct Files {
    size_t count;
    int fd[MAX_FDS];
};

static void close_files(struct Files *files) {
    for (size_t i = 0; i < files->count; i++) close(files->fd[i]);
    files->count = 0;
}

static bool read_all(int fd, void *buf, size_t len) {
    uint8_t *p = (uint8_t *)buf;
    size_t got = 0;
    while (got < len) {
        ssize_t n;
        do {
            n = read(fd, p + got, len - got);
        } while (n < 0 && errno == EINTR);
        if (n <= 0) return false;
        got += (size_t)n;
    }
    return true;
}

static bool send_msg(int fd, const void *data, size_t len, const int *fds, size_t fd_count) {
    union {
        struct cmsghdr align;
        unsigned char data[CMSG_SPACE(MAX_FDS * sizeof(int))];
    } control;
    const uint8_t *p = (const uint8_t *)data;
    size_t sent = 0;
    while (sent < len) {
        struct iovec iov = {.iov_base = (void *)(p + sent), .iov_len = len - sent};
        struct msghdr msg = {.msg_iov = &iov, .msg_iovlen = 1};
        if (sent == 0 && fd_count > 0) {
            msg.msg_control = &control;
            msg.msg_controllen = CMSG_SPACE(fd_count * sizeof(int));
            struct cmsghdr *head = CMSG_FIRSTHDR(&msg);
            head->cmsg_level = SOL_SOCKET;
            head->cmsg_type = SCM_RIGHTS;
            head->cmsg_len = CMSG_LEN(fd_count * sizeof(int));
            memcpy(CMSG_DATA(head), fds, fd_count * sizeof(int));
        }
        ssize_t n;
        do {
            n = sendmsg(fd, &msg, MSG_NOSIGNAL);
        } while (n < 0 && errno == EINTR);
        if (n <= 0) return false;
        sent += (size_t)n;
    }
    return true;
}

static ssize_t recv_seq(int fd, void *data, size_t len, struct Files *files) {
    union {
        struct cmsghdr align;
        unsigned char data[CMSG_SPACE(MAX_FDS * sizeof(int))];
    } control;
    struct iovec iov = {.iov_base = data, .iov_len = len};
    struct msghdr msg = {
        .msg_iov = &iov, .msg_iovlen = 1,
        .msg_control = &control, .msg_controllen = sizeof(control),
    };
    ssize_t n;
    do {
        n = recvmsg(fd, &msg, MSG_CMSG_CLOEXEC);
    } while (n < 0 && errno == EINTR);
    if (n < 0) return n;
    files->count = 0;
    for (struct cmsghdr *head = CMSG_FIRSTHDR(&msg); head; head = CMSG_NXTHDR(&msg, head)) {
        if (head->cmsg_level != SOL_SOCKET || head->cmsg_type != SCM_RIGHTS ||
            head->cmsg_len < CMSG_LEN(0)) continue;
        size_t cnt = (head->cmsg_len - CMSG_LEN(0)) / sizeof(int);
        for (size_t i = 0; i < cnt; i++) {
            int rfd;
            memcpy(&rfd, CMSG_DATA(head) + i * sizeof(int), sizeof(rfd));
            if (files->count < MAX_FDS) files->fd[files->count++] = rfd;
            else close(rfd);
        }
    }
    return (msg.msg_flags & (MSG_TRUNC | MSG_CTRUNC)) ? -1 : n;
}

static bool register_ahb(const uint8_t *meta, size_t meta_len, const int *fds, size_t fd_count,
                         int *out_token_fd, uint64_t *out_reg_id) {
    if (meta_len == 0 || meta_len > MAX_METADATA || fd_count == 0) return false;
    uint8_t req[1 + MAX_METADATA];
    req[0] = 1;
    memcpy(req + 1, meta, meta_len);
    pthread_mutex_lock(&g_ahb_lock);
    bool ok = false;
    if (send_msg(g_ahb_fd, req, 1 + meta_len, fds, fd_count)) {
        uint8_t resp[9];
        struct Files rf = {0};
        ssize_t n = recv_seq(g_ahb_fd, resp, sizeof(resp), &rf);
        if (n == 9 && resp[0] == 1 && rf.count == 1) {
            memcpy(out_reg_id, resp + 1, 8);
            *out_token_fd = rf.fd[0];
            ok = true;
        } else {
            close_files(&rf);
        }
    }
    pthread_mutex_unlock(&g_ahb_lock);
    return ok;
}

static void unregister_ahb(uint64_t reg_id) {
    uint8_t req[9];
    req[0] = 2;
    memcpy(req + 1, &reg_id, 8);
    pthread_mutex_lock(&g_ahb_lock);
    (void)send_msg(g_ahb_fd, req, sizeof(req), NULL, 0);
    pthread_mutex_unlock(&g_ahb_lock);
}

static bool send_frame_packet(int sock, uint32_t kind, const void *payload, uint32_t len,
                              const int *fds, size_t fd_count) {
    uint32_t hdr[2] = {kind, len};
    uint8_t stack_buf[256];
    uint8_t *buf = (len + 8 <= sizeof(stack_buf)) ? stack_buf : malloc(len + 8);
    if (!buf) return false;
    memcpy(buf, hdr, 8);
    if (len > 0 && payload) memcpy(buf + 8, payload, len);
    bool ok = send_msg(sock, buf, len + 8, fds, fd_count);
    if (buf != stack_buf) free(buf);
    return ok;
}

static bool pack_ahb(AHardwareBuffer *ahb, uint8_t *meta, size_t *meta_len, struct Files *files) {
    int pair[2];
    if (socketpair(AF_UNIX, SOCK_SEQPACKET | SOCK_CLOEXEC, 0, pair) < 0) return false;
    bool ok = false;
    if (AHardwareBuffer_sendHandleToUnixSocket(ahb, pair[0]) == 0) {
        ssize_t n = recv_seq(pair[1], meta, MAX_METADATA, files);
        if (n > 0 && files->count > 0) {
            *meta_len = (size_t)n;
            ok = true;
        } else {
            close_files(files);
        }
    }
    close(pair[0]);
    close(pair[1]);
    return ok;
}

static AHardwareBuffer *unpack_ahb(const uint8_t *data, size_t len, const int *fds, size_t count) {
    int pair[2];
    if (socketpair(AF_UNIX, SOCK_SEQPACKET | SOCK_CLOEXEC, 0, pair) < 0) return NULL;
    AHardwareBuffer *ahb = NULL;
    if (send_msg(pair[0], data, len, fds, count) &&
        AHardwareBuffer_recvHandleFromUnixSocket(pair[1], &ahb) != 0) {
        ahb = NULL;
    }
    close(pair[0]);
    close(pair[1]);
    return ahb;
}

/* ==================== Window stream encoders for the remote desktop client ==================== */

#define ENC_HEADER 64
#define ENC_MAX_STREAMS 32
#define ENC_MAX_LAYERS 16
#define ENC_MAX_OUT (4 * 1024 * 1024)
#define ENC_CHUNK_SIZE (48 * 1024)
#define ENC_MAX_W 3840
#define ENC_MAX_H 2160
#define ENC_CROPS 32

struct EncLayer {
    uint32_t id;
    uint32_t flight;
    AHardwareBuffer *ahb;
    EGLImageKHR image;
    GLuint tex;
    int32_t x, y, w, h, z;
    float uv[4];
};

struct EncCrop {
    int64_t pts_us;
    uint32_t w, h;
};

struct EncShared {
    int fd;
    pthread_mutex_t send_lock;
    EGLDisplay dpy;
    EGLConfig cfg;
    EGLContext ctx;
    EGLSurface current;
    GLuint prog;
    GLint u_rect;
    GLint u_uv;
    PFNEGLGETNATIVECLIENTBUFFERANDROIDPROC get_client_buf;
    PFNEGLCREATEIMAGEKHRPROC create_image;
    PFNEGLDESTROYIMAGEKHRPROC destroy_image;
    PFNGLEGLIMAGETARGETTEXTURE2DOESPROC image_target_tex2d;
    PFNEGLCREATESYNCKHRPROC create_sync;
    PFNEGLDESTROYSYNCKHRPROC destroy_sync;
    PFNEGLWAITSYNCKHRPROC wait_sync;
    PFNEGLDUPNATIVEFENCEFDANDROIDPROC dup_fence_fd;
    PFNEGLPRESENTATIONTIMEANDROIDPROC set_pts;
};

struct EncStream {
    bool used;
    uint32_t id;
    struct EncShared *sh;
    AMediaCodec *codec;
    ANativeWindow *win;
    EGLSurface surf;
    int enc_w, enc_h;
    int win_w, win_h;
    int out_w, out_h;
    struct EncLayer layers[ENC_MAX_LAYERS];
    int nlayers;
    pthread_t drain;
    bool draining;
    volatile bool running;
    uint8_t sps[512];
    size_t sps_len;
    pthread_mutex_t crop_lock;
    struct EncCrop crops[ENC_CROPS];
    uint32_t next_crop;
    uint32_t seq;
};

static struct EncStream g_streams[ENC_MAX_STREAMS];

static GLuint compile_shader(GLenum type, const char *src) {
    GLuint sh = glCreateShader(type);
    glShaderSource(sh, 1, &src, NULL);
    glCompileShader(sh);
    GLint ok = 0;
    glGetShaderiv(sh, GL_COMPILE_STATUS, &ok);
    if (!ok) {
        glDeleteShader(sh);
        return 0;
    }
    return sh;
}

static bool enc_send(struct EncShared *sh, const void *data, size_t len, const int *fds, size_t nfds) {
    pthread_mutex_lock(&sh->send_lock);
    bool ok = send_msg(sh->fd, data, len, fds, nfds);
    pthread_mutex_unlock(&sh->send_lock);
    return ok;
}

static void enc_ack(struct EncShared *sh, uint32_t flight, int fence) {
    uint32_t ack[2] = {1, flight};
    int dup_fd = fence >= 0 ? fcntl(fence, F_DUPFD_CLOEXEC, 3) : -1;
    enc_send(sh, ack, sizeof(ack), dup_fd >= 0 ? &dup_fd : NULL, dup_fd >= 0 ? 1 : 0);
    if (dup_fd >= 0) close(dup_fd);
}

static void *enc_drain(void *arg) {
    struct EncStream *s = arg;
    uint8_t *full = malloc(12 + ENC_MAX_OUT);
    uint8_t *chunk = malloc(20 + ENC_CHUNK_SIZE);
    if (!full || !chunk) {
        free(full);
        free(chunk);
        return NULL;
    }
    while (s->running) {
        AMediaCodecBufferInfo info;
        ssize_t out = AMediaCodec_dequeueOutputBuffer(s->codec, &info, 10000);
        if (out < 0) continue;
        size_t bsz = 0;
        uint8_t *ob = AMediaCodec_getOutputBuffer(s->codec, out, &bsz);
        if (ob && info.size > 0 && (size_t)info.offset + (size_t)info.size <= bsz) {
            const uint8_t *src = ob + info.offset;
            size_t len = (size_t)info.size;
            if (info.flags & AMEDIACODEC_BUFFER_FLAG_CODEC_CONFIG) {
                if (len <= sizeof(s->sps)) {
                    memcpy(s->sps, src, len);
                    s->sps_len = len;
                }
            } else {
                bool key = (info.flags & AMEDIACODEC_BUFFER_FLAG_KEY_FRAME) != 0;
                size_t prefix = key ? s->sps_len : 0;
                if (12 + prefix + len <= 12 + (size_t)ENC_MAX_OUT) {
                    uint32_t crop_w = (uint32_t)s->out_w, crop_h = (uint32_t)s->out_h;
                    pthread_mutex_lock(&s->crop_lock);
                    for (uint32_t i = 0; i < ENC_CROPS; i++) {
                        if (s->crops[i].pts_us == info.presentationTimeUs && s->crops[i].w) {
                            crop_w = s->crops[i].w;
                            crop_h = s->crops[i].h;
                            break;
                        }
                    }
                    pthread_mutex_unlock(&s->crop_lock);
                    uint32_t head[3] = {key ? 1u : 0u, crop_w, crop_h};
                    memcpy(full, head, 12);
                    if (prefix) memcpy(full + 12, s->sps, prefix);
                    memcpy(full + 12 + prefix, src, len);
                    uint32_t total = (uint32_t)(12 + prefix + len);
                    s->seq++;
                    for (uint32_t pos = 0; pos < total; pos += ENC_CHUNK_SIZE) {
                        uint32_t n = total - pos;
                        if (n > ENC_CHUNK_SIZE) n = ENC_CHUNK_SIZE;
                        uint32_t hdr[5] = {2, s->id, s->seq, total, pos};
                        memcpy(chunk, hdr, 20);
                        memcpy(chunk + 20, full + pos, n);
                        if (!enc_send(s->sh, chunk, 20 + n, NULL, 0)) break;
                    }
                }
            }
        }
        AMediaCodec_releaseOutputBuffer(s->codec, out, false);
    }
    free(chunk);
    free(full);
    return NULL;
}

static bool enc_current(struct EncShared *sh, EGLSurface surf) {
    if (sh->current == surf) return true;
    if (!eglMakeCurrent(sh->dpy, surf, surf, sh->ctx)) return false;
    sh->current = surf;
    return true;
}

static void enc_stop_codec(struct EncStream *s) {
    struct EncShared *sh = s->sh;
    if (s->draining) {
        s->running = false;
        pthread_join(s->drain, NULL);
        s->draining = false;
    }
    if (s->surf != EGL_NO_SURFACE) {
        if (sh->current == s->surf) {
            eglMakeCurrent(sh->dpy, EGL_NO_SURFACE, EGL_NO_SURFACE, EGL_NO_CONTEXT);
            sh->current = EGL_NO_SURFACE;
        }
        eglDestroySurface(sh->dpy, s->surf);
        s->surf = EGL_NO_SURFACE;
    }
    if (s->codec) {
        AMediaCodec_stop(s->codec);
        AMediaCodec_delete(s->codec);
        s->codec = NULL;
    }
    if (s->win) {
        ANativeWindow_release(s->win);
        s->win = NULL;
    }
    s->enc_w = s->enc_h = 0;
    s->sps_len = 0;
}

#define ENC_PIXEL_RATE 480000000LL

static int round_up(int v, int step) {
    return (v + step - 1) / step * step;
}

static bool enc_start_codec(struct EncStream *s, int w, int h) {
    struct EncShared *sh = s->sh;
    int ew = round_up(w, 64), eh = round_up(h, 64);
    if (ew > ENC_MAX_W) ew = ENC_MAX_W;
    if (eh > ENC_MAX_H) eh = ENC_MAX_H;
    if (s->codec && ew <= s->enc_w && eh <= s->enc_h &&
        (int64_t)ew * eh * 2 > (int64_t)s->enc_w * s->enc_h) {
        return true;
    }
    enc_stop_codec(s);
    int64_t rate = (int64_t)ew * eh * 12;
    if (rate < 4000000) rate = 4000000;
    if (rate > 80000000) rate = 80000000;
    int32_t fast_rate = (int32_t)(ENC_PIXEL_RATE / ((int64_t)ew * eh));
    if (fast_rate > 240) fast_rate = 240;
    if (fast_rate < 60) fast_rate = 60;
    AMediaCodec *codec = NULL;
    ANativeWindow *win = NULL;
    for (int attempt = 0; attempt < 3; attempt++) {
        int32_t operating = attempt == 0 ? fast_rate : attempt == 1 ? fast_rate / 2 : 0;
        codec = AMediaCodec_createEncoderByType("video/avc");
        if (!codec) continue;
        AMediaFormat *fmt = AMediaFormat_new();
        AMediaFormat_setString(fmt, AMEDIAFORMAT_KEY_MIME, "video/avc");
        AMediaFormat_setInt32(fmt, AMEDIAFORMAT_KEY_WIDTH, ew);
        AMediaFormat_setInt32(fmt, AMEDIAFORMAT_KEY_HEIGHT, eh);
        AMediaFormat_setInt32(fmt, AMEDIAFORMAT_KEY_BIT_RATE, (int32_t)rate);
        AMediaFormat_setInt32(fmt, AMEDIAFORMAT_KEY_FRAME_RATE, operating ? 60 : 30);
        if (operating) AMediaFormat_setInt32(fmt, "operating-rate", operating);
        AMediaFormat_setInt32(fmt, AMEDIAFORMAT_KEY_I_FRAME_INTERVAL, 10);
        AMediaFormat_setInt32(fmt, AMEDIAFORMAT_KEY_COLOR_FORMAT, 0x7f000789);
        AMediaFormat_setInt32(fmt, "max-bframes", 0);
        AMediaFormat_setInt32(fmt, "latency", 1);
        AMediaFormat_setInt32(fmt, "priority", 0);
        AMediaFormat_setInt32(fmt, "bitrate-mode", 1);
        AMediaFormat_setInt32(fmt, "video-qp-i-max", 22);
        AMediaFormat_setInt32(fmt, "video-qp-p-max", 24);
        media_status_t st = AMediaCodec_configure(codec, fmt, NULL, NULL, AMEDIACODEC_CONFIGURE_FLAG_ENCODE);
        AMediaFormat_delete(fmt);
        if (st == AMEDIA_OK) st = AMediaCodec_createInputSurface(codec, &win);
        if (st == AMEDIA_OK) st = AMediaCodec_start(codec);
        if (st == AMEDIA_OK && win) break;
        if (win) ANativeWindow_release(win);
        win = NULL;
        AMediaCodec_delete(codec);
        codec = NULL;
    }
    if (!codec) return false;
    EGLSurface surf = eglCreateWindowSurface(sh->dpy, sh->cfg, win, NULL);
    if (surf == EGL_NO_SURFACE) {
        ANativeWindow_release(win);
        AMediaCodec_stop(codec);
        AMediaCodec_delete(codec);
        return false;
    }
    s->codec = codec;
    s->win = win;
    s->surf = surf;
    s->enc_w = ew;
    s->enc_h = eh;
    s->running = true;
    if (pthread_create(&s->drain, NULL, enc_drain, s) != 0) {
        s->running = false;
        enc_stop_codec(s);
        return false;
    }
    s->draining = true;
    if (enc_current(sh, surf)) eglSwapInterval(sh->dpy, 0);
    return true;
}

static void enc_drop_layer(struct EncShared *sh, struct EncLayer *l) {
    if (l->image != EGL_NO_IMAGE_KHR) sh->destroy_image(sh->dpy, l->image);
    if (l->tex) glDeleteTextures(1, &l->tex);
    if (l->ahb) AHardwareBuffer_release(l->ahb);
    memset(l, 0, sizeof(*l));
}

static int enc_render(struct EncStream *s) {
    struct EncShared *sh = s->sh;
    if (!s->codec || !enc_current(sh, s->surf)) return -1;
    glDisable(GL_SCISSOR_TEST);
    glViewport(0, 0, s->enc_w, s->enc_h);
    glClearColor(0, 0, 0, 1);
    glClear(GL_COLOR_BUFFER_BIT);
    glViewport(0, s->enc_h - s->out_h, s->out_w, s->out_h);
    glUseProgram(sh->prog);
    static const GLfloat quad[8] = {0, 0, 1, 0, 0, 1, 1, 1};
    glEnableVertexAttribArray(0);
    glVertexAttribPointer(0, 2, GL_FLOAT, GL_FALSE, 0, quad);
    int order[ENC_MAX_LAYERS];
    for (int i = 0; i < s->nlayers; i++) order[i] = i;
    for (int i = 1; i < s->nlayers; i++) {
        for (int j = i; j > 0 && s->layers[order[j]].z < s->layers[order[j - 1]].z; j--) {
            int t = order[j];
            order[j] = order[j - 1];
            order[j - 1] = t;
        }
    }
    glBlendFunc(GL_ONE, GL_ONE_MINUS_SRC_ALPHA);
    for (int i = 0; i < s->nlayers; i++) {
        struct EncLayer *l = &s->layers[order[i]];
        if (!l->tex) continue;
        if (i == 0) glDisable(GL_BLEND);
        else glEnable(GL_BLEND);
        float x0 = (float)l->x / s->win_w * 2.0f - 1.0f;
        float x1 = (float)(l->x + l->w) / s->win_w * 2.0f - 1.0f;
        float y0 = 1.0f - (float)l->y / s->win_h * 2.0f;
        float y1 = 1.0f - (float)(l->y + l->h) / s->win_h * 2.0f;
        glUniform4f(sh->u_rect, x0, y0, x1, y1);
        glUniform4f(sh->u_uv, l->uv[0], l->uv[1], l->uv[2], l->uv[3]);
        glBindTexture(GL_TEXTURE_EXTERNAL_OES, l->tex);
        glDrawArrays(GL_TRIANGLE_STRIP, 0, 4);
    }
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    int64_t now_ns = (int64_t)ts.tv_sec * 1000000000LL + ts.tv_nsec;
    if (sh->set_pts) sh->set_pts(sh->dpy, s->surf, now_ns);
    pthread_mutex_lock(&s->crop_lock);
    s->crops[s->next_crop % ENC_CROPS] = (struct EncCrop){now_ns / 1000, (uint32_t)s->out_w, (uint32_t)s->out_h};
    s->next_crop++;
    pthread_mutex_unlock(&s->crop_lock);
    EGLSyncKHR sync = EGL_NO_SYNC_KHR;
    if (sh->create_sync && sh->dup_fence_fd) {
        EGLint attr[] = {EGL_SYNC_NATIVE_FENCE_FD_ANDROID, EGL_NO_NATIVE_FENCE_FD_ANDROID, EGL_NONE};
        sync = sh->create_sync(sh->dpy, EGL_SYNC_NATIVE_FENCE_ANDROID, attr);
    }
    eglSwapBuffers(sh->dpy, s->surf);
    int fence = -1;
    if (sync != EGL_NO_SYNC_KHR) {
        fence = sh->dup_fence_fd(sh->dpy, sync);
        sh->destroy_sync(sh->dpy, sync);
    }
    if (fence < 0) glFinish();
    return fence;
}

static void enc_resize(struct EncStream *s, int win_w, int win_h) {
    if (win_w < 16) win_w = 16;
    if (win_h < 16) win_h = 16;
    s->win_w = win_w;
    s->win_h = win_h;
    int out_w = win_w, out_h = win_h;
    if (out_w > ENC_MAX_W || out_h > ENC_MAX_H) {
        double k = (double)ENC_MAX_W / out_w;
        if ((double)ENC_MAX_H / out_h < k) k = (double)ENC_MAX_H / out_h;
        out_w = (int)(out_w * k);
        out_h = (int)(out_h * k);
    }
    s->out_w = out_w & ~1;
    s->out_h = out_h & ~1;
    enc_start_codec(s, s->out_w, s->out_h);
}

static struct EncStream *enc_stream(struct EncShared *sh, uint32_t id, bool create) {
    struct EncStream *free_slot = NULL;
    for (int i = 0; i < ENC_MAX_STREAMS; i++) {
        if (g_streams[i].used && g_streams[i].id == id) return &g_streams[i];
        if (!g_streams[i].used && !free_slot) free_slot = &g_streams[i];
    }
    if (!create || !free_slot) return NULL;
    memset(free_slot, 0, sizeof(*free_slot));
    free_slot->used = true;
    free_slot->id = id;
    free_slot->sh = sh;
    free_slot->surf = EGL_NO_SURFACE;
    pthread_mutex_init(&free_slot->crop_lock, NULL);
    return free_slot;
}

static void enc_close(struct EncStream *s) {
    struct EncShared *sh = s->sh;
    bool rendered = s->nlayers > 0 && s->codec;
    if (rendered) glFinish();
    for (int i = 0; i < s->nlayers; i++) {
        enc_ack(sh, s->layers[i].flight, -1);
        enc_drop_layer(sh, &s->layers[i]);
    }
    s->nlayers = 0;
    enc_stop_codec(s);
    pthread_mutex_destroy(&s->crop_lock);
    s->used = false;
}

static struct EncLayer *enc_layer(struct EncStream *s, uint32_t id) {
    for (int i = 0; i < s->nlayers; i++) {
        if (s->layers[i].id == id) return &s->layers[i];
    }
    return NULL;
}

static bool enc_setup(struct EncShared *sh) {
    sh->dpy = eglGetDisplay(EGL_DEFAULT_DISPLAY);
    if (sh->dpy == EGL_NO_DISPLAY || !eglInitialize(sh->dpy, NULL, NULL)) return false;
    EGLint cfg_attr[] = {
        EGL_SURFACE_TYPE, EGL_WINDOW_BIT,
        EGL_RENDERABLE_TYPE, EGL_OPENGL_ES2_BIT,
        EGL_RED_SIZE, 8, EGL_GREEN_SIZE, 8, EGL_BLUE_SIZE, 8,
        EGL_RECORDABLE_ANDROID, 1,
        EGL_NONE
    };
    EGLint ncfg = 0;
    if (!eglChooseConfig(sh->dpy, cfg_attr, &sh->cfg, 1, &ncfg) || ncfg < 1) return false;
    EGLint ctx_attr[] = {EGL_CONTEXT_CLIENT_VERSION, 2, EGL_NONE};
    sh->ctx = eglCreateContext(sh->dpy, sh->cfg, EGL_NO_CONTEXT, ctx_attr);
    if (sh->ctx == EGL_NO_CONTEXT) return false;
    if (!eglMakeCurrent(sh->dpy, EGL_NO_SURFACE, EGL_NO_SURFACE, sh->ctx)) return false;
    sh->current = EGL_NO_SURFACE;
    static const char *vs_src =
        "attribute vec2 aPos;\n"
        "uniform vec4 uRect;\n"
        "uniform vec4 uUv;\n"
        "varying vec2 vUv;\n"
        "void main() {\n"
        "  vUv = mix(uUv.xy, uUv.zw, aPos);\n"
        "  gl_Position = vec4(mix(uRect.x, uRect.z, aPos.x), mix(uRect.y, uRect.w, aPos.y), 0.0, 1.0);\n"
        "}\n";
    static const char *fs_src =
        "#extension GL_OES_EGL_image_external : require\n"
        "precision mediump float;\n"
        "varying vec2 vUv;\n"
        "uniform samplerExternalOES uTex;\n"
        "void main() { gl_FragColor = texture2D(uTex, vUv); }\n";
    GLuint vs = compile_shader(GL_VERTEX_SHADER, vs_src);
    GLuint fs = compile_shader(GL_FRAGMENT_SHADER, fs_src);
    if (!vs || !fs) return false;
    sh->prog = glCreateProgram();
    glAttachShader(sh->prog, vs);
    glAttachShader(sh->prog, fs);
    glBindAttribLocation(sh->prog, 0, "aPos");
    glLinkProgram(sh->prog);
    glDeleteShader(vs);
    glDeleteShader(fs);
    sh->u_rect = glGetUniformLocation(sh->prog, "uRect");
    sh->u_uv = glGetUniformLocation(sh->prog, "uUv");
    sh->get_client_buf = (PFNEGLGETNATIVECLIENTBUFFERANDROIDPROC)eglGetProcAddress("eglGetNativeClientBufferANDROID");
    sh->create_image = (PFNEGLCREATEIMAGEKHRPROC)eglGetProcAddress("eglCreateImageKHR");
    sh->destroy_image = (PFNEGLDESTROYIMAGEKHRPROC)eglGetProcAddress("eglDestroyImageKHR");
    sh->image_target_tex2d = (PFNGLEGLIMAGETARGETTEXTURE2DOESPROC)eglGetProcAddress("glEGLImageTargetTexture2DOES");
    sh->create_sync = (PFNEGLCREATESYNCKHRPROC)eglGetProcAddress("eglCreateSyncKHR");
    sh->destroy_sync = (PFNEGLDESTROYSYNCKHRPROC)eglGetProcAddress("eglDestroySyncKHR");
    sh->wait_sync = (PFNEGLWAITSYNCKHRPROC)eglGetProcAddress("eglWaitSyncKHR");
    sh->dup_fence_fd = (PFNEGLDUPNATIVEFENCEFDANDROIDPROC)eglGetProcAddress("eglDupNativeFenceFDANDROID");
    sh->set_pts = (PFNEGLPRESENTATIONTIMEANDROIDPROC)eglGetProcAddress("eglPresentationTimeANDROID");
    return sh->get_client_buf && sh->create_image && sh->destroy_image && sh->image_target_tex2d;
}

static void enc_source(struct EncLayer *l, const uint32_t *s) {
    for (int i = 0; i < 4; i++) l->uv[i] = (float)s[i] / 65536.0f;
}

static void enc_present(struct EncShared *sh, const uint32_t *f, const uint8_t *meta, size_t meta_len,
                        const int *fds, size_t nfds, int acquire) {
    uint32_t flight = f[1];
    struct EncStream *s = enc_stream(sh, f[2], true);
    AHardwareBuffer *ahb = s ? unpack_ahb(meta, meta_len, fds, nfds) : NULL;
    EGLImageKHR image = EGL_NO_IMAGE_KHR;
    if (ahb) {
        EGLClientBuffer cb = sh->get_client_buf(ahb);
        EGLint attr[] = {EGL_IMAGE_PRESERVED_KHR, EGL_TRUE, EGL_NONE};
        if (cb) image = sh->create_image(sh->dpy, EGL_NO_CONTEXT, EGL_NATIVE_BUFFER_ANDROID, cb, attr);
    }
    struct EncLayer *l = image != EGL_NO_IMAGE_KHR ? enc_layer(s, f[3]) : NULL;
    if (image != EGL_NO_IMAGE_KHR && !l && s->nlayers < ENC_MAX_LAYERS) {
        l = &s->layers[s->nlayers++];
        memset(l, 0, sizeof(*l));
        l->id = f[3];
    }
    if (!l) {
        if (image != EGL_NO_IMAGE_KHR) sh->destroy_image(sh->dpy, image);
        if (ahb) AHardwareBuffer_release(ahb);
        if (acquire >= 0) close(acquire);
        enc_ack(sh, flight, -1);
        return;
    }
    enc_resize(s, (int)f[8], (int)f[9]);
    if (acquire >= 0) {
        bool waited = false;
        if (sh->create_sync && sh->wait_sync && enc_current(sh, s->surf != EGL_NO_SURFACE ? s->surf : EGL_NO_SURFACE)) {
            EGLint attr[] = {EGL_SYNC_NATIVE_FENCE_FD_ANDROID, acquire, EGL_NONE};
            EGLSyncKHR sync = sh->create_sync(sh->dpy, EGL_SYNC_NATIVE_FENCE_ANDROID, attr);
            if (sync != EGL_NO_SYNC_KHR) {
                sh->wait_sync(sh->dpy, sync, 0);
                sh->destroy_sync(sh->dpy, sync);
                waited = true;
            }
        }
        if (!waited) {
            struct pollfd pfd = {.fd = acquire, .events = POLLIN};
            poll(&pfd, 1, 50);
            close(acquire);
        }
    }
    uint32_t old_flight = l->flight;
    AHardwareBuffer *old_ahb = l->ahb;
    EGLImageKHR old_image = l->image;
    bool had_old = old_ahb != NULL;
    enc_current(sh, s->surf);
    if (!l->tex) {
        glGenTextures(1, &l->tex);
        glBindTexture(GL_TEXTURE_EXTERNAL_OES, l->tex);
        glTexParameteri(GL_TEXTURE_EXTERNAL_OES, GL_TEXTURE_MIN_FILTER, GL_LINEAR);
        glTexParameteri(GL_TEXTURE_EXTERNAL_OES, GL_TEXTURE_MAG_FILTER, GL_LINEAR);
        glTexParameteri(GL_TEXTURE_EXTERNAL_OES, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE);
        glTexParameteri(GL_TEXTURE_EXTERNAL_OES, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE);
    }
    glBindTexture(GL_TEXTURE_EXTERNAL_OES, l->tex);
    sh->image_target_tex2d(GL_TEXTURE_EXTERNAL_OES, image);
    l->ahb = ahb;
    l->image = image;
    l->flight = flight;
    l->x = (int32_t)f[4];
    l->y = (int32_t)f[5];
    l->w = (int32_t)f[6];
    l->h = (int32_t)f[7];
    l->z = (int32_t)f[10];
    enc_source(l, f + 12);
    int fence = enc_render(s);
    if (had_old) {
        enc_ack(sh, old_flight, fence);
        sh->destroy_image(sh->dpy, old_image);
        AHardwareBuffer_release(old_ahb);
    }
    if (fence >= 0) close(fence);
}

static void enc_remove_layer(struct EncShared *sh, uint32_t stream, uint32_t layer) {
    struct EncStream *s = enc_stream(sh, stream, false);
    struct EncLayer *l = s ? enc_layer(s, layer) : NULL;
    if (!l) return;
    struct EncLayer gone = *l;
    *l = s->layers[--s->nlayers];
    int fence = enc_render(s);
    enc_ack(sh, gone.flight, fence);
    if (fence >= 0) close(fence);
    enc_current(sh, s->surf);
    enc_drop_layer(sh, &gone);
}

static void enc_relayout(struct EncShared *sh, const uint32_t *f) {
    struct EncStream *s = enc_stream(sh, f[1], false);
    struct EncLayer *l = s ? enc_layer(s, f[2]) : NULL;
    if (!l) return;
    l->x = (int32_t)f[3];
    l->y = (int32_t)f[4];
    l->w = (int32_t)f[5];
    l->h = (int32_t)f[6];
    l->z = (int32_t)f[9];
    enc_source(l, f + 10);
    enc_resize(s, (int)f[7], (int)f[8]);
    int fence = enc_render(s);
    if (fence >= 0) close(fence);
}

static void *encoder_thread(void *arg) {
    int fd = (int)(intptr_t)arg;
    int bufsz = 4 * 1024 * 1024;
    setsockopt(fd, SOL_SOCKET, SO_SNDBUF, &bufsz, sizeof(bufsz));
    setsockopt(fd, SOL_SOCKET, SO_RCVBUF, &bufsz, sizeof(bufsz));
    struct EncShared sh;
    memset(&sh, 0, sizeof(sh));
    sh.fd = fd;
    pthread_mutex_init(&sh.send_lock, NULL);
    bool ready = enc_setup(&sh);
    for (;;) {
        uint8_t packet[ENC_HEADER + MAX_METADATA];
        struct Files files = {0};
        ssize_t n = recv_seq(fd, packet, sizeof(packet), &files);
        if (n <= 0) {
            close_files(&files);
            break;
        }
        uint32_t f[16] = {0};
        memcpy(f, packet, (size_t)n < sizeof(f) ? (size_t)n : sizeof(f));
        if (f[0] == 3) {
            close_files(&files);
            break;
        }
        if ((f[0] == 1 || f[0] == 7) && n > ENC_HEADER) {
            bool fenced = f[0] == 1;
            if (!ready || files.count < (fenced ? 2u : 1u)) {
                close_files(&files);
                enc_ack(&sh, f[1], -1);
                continue;
            }
            int acquire = fenced ? files.fd[--files.count] : -1;
            enc_present(&sh, f, packet + ENC_HEADER, (size_t)n - ENC_HEADER, files.fd, files.count, acquire);
            close_files(&files);
            continue;
        }
        close_files(&files);
        if (f[0] == 2 && n == 4) {
            for (int i = 0; i < ENC_MAX_STREAMS; i++) {
                if (g_streams[i].used) enc_close(&g_streams[i]);
            }
        } else if (f[0] == 2 && n == 8) {
            struct EncStream *s = enc_stream(&sh, f[1], false);
            if (s) enc_close(s);
        } else if (f[0] == 4 && n == 12) {
            enc_remove_layer(&sh, f[1], f[2]);
        } else if (f[0] == 5 && n == 56) {
            enc_relayout(&sh, f);
        } else if (f[0] == 8 && n == 8) {
            struct EncStream *s = enc_stream(&sh, f[1], false);
            if (s && s->codec) {
                AMediaFormat *params = AMediaFormat_new();
                AMediaFormat_setInt32(params, "request-sync", 0);
                AMediaCodec_setParameters(s->codec, params);
                AMediaFormat_delete(params);
            }
        }
    }
    for (int i = 0; i < ENC_MAX_STREAMS; i++) {
        if (g_streams[i].used) enc_close(&g_streams[i]);
    }
    if (sh.dpy != EGL_NO_DISPLAY) {
        eglMakeCurrent(sh.dpy, EGL_NO_SURFACE, EGL_NO_SURFACE, EGL_NO_CONTEXT);
        if (sh.ctx != EGL_NO_CONTEXT) eglDestroyContext(sh.dpy, sh.ctx);
        eglTerminate(sh.dpy);
    }
    pthread_mutex_destroy(&sh.send_lock);
    close(fd);
    return NULL;
}

/* ==================== Hardware Video Decoder Server (bareboot-media-0) ==================== */

struct FrameSlot {
    bool used;
    uint32_t frame_id;
    AImage *image;
    uint64_t reg_id;
};

struct Session {
    int sock;
    AMediaCodec *codec;
    AImageReader *reader;
    uint32_t width;
    uint32_t height;
    uint32_t next_frame_id;
    bool yuv_mode;
    int shm_fd;
    uint8_t *shm_ptr;
    size_t shm_size;
    uint32_t next_shm_slot;
    struct FrameSlot slots[MAX_SLOTS];
};

static void release_slot(struct FrameSlot *slot) {
    if (!slot->used) return;
    if (slot->reg_id) unregister_ahb(slot->reg_id);
    if (slot->image) AImage_deleteAsync(slot->image, -1);
    memset(slot, 0, sizeof(*slot));
}

static void session_cleanup(struct Session *s) {
    for (size_t i = 0; i < MAX_SLOTS; i++) release_slot(&s->slots[i]);
    if (s->codec) {
        AMediaCodec_stop(s->codec);
        AMediaCodec_delete(s->codec);
        s->codec = NULL;
    }
    if (s->reader) {
        AImageReader_delete(s->reader);
        s->reader = NULL;
    }
    if (s->shm_ptr && s->shm_ptr != MAP_FAILED) {
        munmap(s->shm_ptr, s->shm_size);
        s->shm_ptr = NULL;
    }
    if (s->shm_fd >= 0) {
        close(s->shm_fd);
        s->shm_fd = -1;
    }
}

static const char *codec_mime(uint32_t codec_id) {
    switch (codec_id) {
    case 1: return "video/av01";
    case 2: return "video/x-vnd.on2.vp9";
    case 3: return "video/avc";
    case 4: return "video/hevc";
    default: return NULL;
    }
}

static struct FrameSlot *alloc_slot(struct Session *s) {
    for (size_t i = 0; i < MAX_SLOTS; i++) {
        if (!s->slots[i].used) return &s->slots[i];
    }
    return NULL;
}

static bool drain_outputs(struct Session *s, uint32_t *emitted, uint32_t *eos_reached,
                          int64_t timeout_us) {
    for (int iter = 0; iter < 16; iter++) {
        AMediaCodecBufferInfo info;
        ssize_t out = AMediaCodec_dequeueOutputBuffer(s->codec, &info, iter == 0 ? timeout_us : 0);
        if (out == AMEDIACODEC_INFO_OUTPUT_FORMAT_CHANGED) {
            AMediaFormat *of = AMediaCodec_getOutputFormat(s->codec);
            if (of) {
                int32_t w = 0, h = 0;
                if (AMediaFormat_getInt32(of, AMEDIAFORMAT_KEY_WIDTH, &w) && w > 0) s->width = (uint32_t)w;
                if (AMediaFormat_getInt32(of, AMEDIAFORMAT_KEY_HEIGHT, &h) && h > 0) s->height = (uint32_t)h;
                AMediaFormat_delete(of);
            }
            continue;
        }
        if (out == AMEDIACODEC_INFO_TRY_AGAIN_LATER || out == AMEDIACODEC_INFO_OUTPUT_BUFFERS_CHANGED) {
            break;
        }
        if (out < 0) return false;
        bool render = info.size > 0;
        if (info.flags & AMEDIACODEC_BUFFER_FLAG_END_OF_STREAM) *eos_reached = 1;
        AMediaCodec_releaseOutputBuffer(s->codec, out, render);
        if (!render) {
            if (*eos_reached) break;
            continue;
        }
        AImage *img = NULL;
        int fence = -1;
        media_status_t ast = AMEDIA_IMGREADER_NO_BUFFER_AVAILABLE;
        for (int tries = 0; tries < 40; tries++) {
            ast = AImageReader_acquireNextImageAsync(s->reader, &img, &fence);
            if (ast == AMEDIA_OK && img) break;
            usleep(2000);
        }
        if (ast != AMEDIA_OK || !img) {
            if (fence >= 0) close(fence);
            continue;
        }
        AHardwareBuffer *ahb = NULL;
        if (AImage_getHardwareBuffer(img, &ahb) != AMEDIA_OK || !ahb) {
            AImage_deleteAsync(img, fence);
            continue;
        }
        AHardwareBuffer_Desc desc;
        AHardwareBuffer_describe(ahb, &desc);
        uint8_t meta[MAX_METADATA];
        size_t meta_len = 0;
        struct Files files = {0};
        struct FrameSlot *slot = alloc_slot(s);
        int token_fd = -1;
        uint64_t reg_id = 0;
        if (!slot || !pack_ahb(ahb, meta, &meta_len, &files) ||
            !register_ahb(meta, meta_len, files.fd, files.count, &token_fd, &reg_id)) {
            close_files(&files);
            AImage_deleteAsync(img, fence);
            continue;
        }
        close_files(&files);
        uint32_t fid = ++s->next_frame_id;
        if (fid == 0) fid = ++s->next_frame_id;
        slot->used = true;
        slot->frame_id = fid;
        slot->image = img;
        slot->reg_id = reg_id;

        uint8_t payload[24];
        uint32_t has_fence = fence >= 0 ? 1 : 0;
        memcpy(payload + 0, &fid, 4);
        memcpy(payload + 4, &desc.width, 4);
        memcpy(payload + 8, &desc.height, 4);
        memcpy(payload + 12, &info.presentationTimeUs, 8);
        memcpy(payload + 20, &has_fence, 4);
        int send_fds[2] = {token_fd, fence};
        bool ok = send_frame_packet(s->sock, 2, payload, sizeof(payload),
                                    send_fds, has_fence ? 2 : 1);
        close(token_fd);
        if (fence >= 0) close(fence);
        if (!ok) {
            release_slot(slot);
            return false;
        }
        (*emitted)++;
        if (*eos_reached) break;
    }
    return true;
}

static bool drain_outputs_yuv(struct Session *s, uint32_t *emitted, uint32_t *eos_reached,
                              int64_t timeout_us) {
    for (int iter = 0; iter < 16; iter++) {
        AMediaCodecBufferInfo info;
        ssize_t out = AMediaCodec_dequeueOutputBuffer(s->codec, &info, iter == 0 ? timeout_us : 0);
        if (out == AMEDIACODEC_INFO_OUTPUT_FORMAT_CHANGED) {
            AMediaFormat *of = AMediaCodec_getOutputFormat(s->codec);
            if (of) {
                int32_t w = 0, h = 0;
                if (AMediaFormat_getInt32(of, AMEDIAFORMAT_KEY_WIDTH, &w) && w > 0) s->width = (uint32_t)w;
                if (AMediaFormat_getInt32(of, AMEDIAFORMAT_KEY_HEIGHT, &h) && h > 0) s->height = (uint32_t)h;
                AMediaFormat_delete(of);
            }
            continue;
        }
        if (out == AMEDIACODEC_INFO_TRY_AGAIN_LATER || out == AMEDIACODEC_INFO_OUTPUT_BUFFERS_CHANGED) {
            break;
        }
        if (out < 0) return false;
        bool render = info.size > 0;
        if (info.flags & AMEDIACODEC_BUFFER_FLAG_END_OF_STREAM) *eos_reached = 1;
        AMediaCodec_releaseOutputBuffer(s->codec, out, render);
        if (!render) {
            if (*eos_reached) break;
            continue;
        }
        AImage *img = NULL;
        int fence = -1;
        media_status_t ast = AMEDIA_IMGREADER_NO_BUFFER_AVAILABLE;
        for (int tries = 0; tries < 40; tries++) {
            ast = AImageReader_acquireNextImageAsync(s->reader, &img, &fence);
            if (ast == AMEDIA_OK && img) break;
            usleep(200);
        }
        if (ast != AMEDIA_OK || !img) {
            if (fence >= 0) close(fence);
            continue;
        }
        if (fence >= 0) {
            struct pollfd pfd = {.fd = fence, .events = POLLIN};
            poll(&pfd, 1, 200);
            close(fence);
        }
        int32_t iw = 0, ih = 0;
        AImage_getWidth(img, &iw);
        AImage_getHeight(img, &ih);
        AImageCropRect crop = {0, 0, iw, ih};
        AImage_getCropRect(img, &crop);
        int32_t cx = crop.left >= 0 && crop.left < iw ? crop.left : 0;
        int32_t cy = crop.top >= 0 && crop.top < ih ? crop.top : 0;
        int32_t cr = crop.right > cx && crop.right <= iw ? crop.right : iw;
        int32_t cb = crop.bottom > cy && crop.bottom <= ih ? crop.bottom : ih;
        uint32_t w = (uint32_t)(cr - cx);
        uint32_t h = (uint32_t)(cb - cy);
        if (w < 2 || h < 2 || w > 4096 || h > 4096) {
            AImage_delete(img);
            continue;
        }
        uint8_t *y_data = NULL, *u_data = NULL, *v_data = NULL;
        int y_len = 0, u_len = 0, v_len = 0;
        int32_t y_rs = 0, u_rs = 0, v_rs = 0;
        int32_t u_ps = 0, v_ps = 0;
        if (AImage_getPlaneData(img, 0, &y_data, &y_len) != AMEDIA_OK ||
            AImage_getPlaneData(img, 1, &u_data, &u_len) != AMEDIA_OK ||
            AImage_getPlaneData(img, 2, &v_data, &v_len) != AMEDIA_OK ||
            AImage_getPlaneRowStride(img, 0, &y_rs) != AMEDIA_OK ||
            AImage_getPlaneRowStride(img, 1, &u_rs) != AMEDIA_OK ||
            AImage_getPlaneRowStride(img, 2, &v_rs) != AMEDIA_OK ||
            AImage_getPlanePixelStride(img, 1, &u_ps) != AMEDIA_OK ||
            AImage_getPlanePixelStride(img, 2, &v_ps) != AMEDIA_OK ||
            !y_data || !u_data || !v_data || y_rs < (int32_t)w || u_ps < 1 || v_ps < 1) {
            AImage_delete(img);
            continue;
        }
        uint32_t y_stride = (w + 15u) & ~15u;
        uint32_t cw = (w + 1u) >> 1;
        uint32_t ch = (h + 1u) >> 1;
        uint32_t c_stride = (cw + 15u) & ~15u;
        uint32_t y_off = 0;
        uint32_t u_off = (y_stride * h + 4095u) & ~4095u;
        uint32_t v_off = (u_off + c_stride * ch + 4095u) & ~4095u;
        if ((size_t)v_off + (size_t)c_stride * ch > YUV_SLOT_BYTES) {
            AImage_delete(img);
            continue;
        }
        uint32_t slot = s->next_shm_slot++ % YUV_SHM_SLOTS;
        uint8_t *base = s->shm_ptr + (size_t)slot * YUV_SLOT_BYTES;
        uint8_t *dst_y = base + y_off;
        for (uint32_t r = 0; r < h; r++) {
            memcpy(dst_y + (size_t)r * y_stride, y_data + (size_t)(cy + (int32_t)r) * (size_t)y_rs + (size_t)cx, w);
        }
        uint8_t *dst_u = base + u_off;
        uint8_t *dst_v = base + v_off;
        int32_t ccy = cy >> 1;
        int32_t ccx = cx >> 1;
        for (uint32_t r = 0; r < ch; r++) {
            uint8_t *du = dst_u + (size_t)r * c_stride;
            uint8_t *dv = dst_v + (size_t)r * c_stride;
            const uint8_t *ur = u_data + (size_t)(ccy + (int32_t)r) * (size_t)u_rs + (size_t)ccx * (size_t)u_ps;
            const uint8_t *vr = v_data + (size_t)(ccy + (int32_t)r) * (size_t)v_rs + (size_t)ccx * (size_t)v_ps;
            if (u_ps == 1 && v_ps == 1) {
                memcpy(du, ur, cw);
                memcpy(dv, vr, cw);
            } else if (u_ps == 2 && v_ps == 2 && vr == ur + 1) {
                for (uint32_t x = 0; x < cw; x++) {
                    du[x] = ur[x * 2u];
                    dv[x] = ur[x * 2u + 1u];
                }
            } else {
                for (uint32_t x = 0; x < cw; x++) {
                    du[x] = ur[x * (uint32_t)u_ps];
                    dv[x] = vr[x * (uint32_t)v_ps];
                }
            }
        }
        AImage_delete(img);

        uint8_t payload[40];
        memcpy(payload + 0, &slot, 4);
        memcpy(payload + 4, &w, 4);
        memcpy(payload + 8, &h, 4);
        memcpy(payload + 12, &y_off, 4);
        memcpy(payload + 16, &y_stride, 4);
        memcpy(payload + 20, &u_off, 4);
        memcpy(payload + 24, &v_off, 4);
        memcpy(payload + 28, &c_stride, 4);
        memcpy(payload + 32, &info.presentationTimeUs, 8);
        if (!send_frame_packet(s->sock, 12, payload, sizeof(payload), NULL, 0)) {
            return false;
        }
        (*emitted)++;
        if (*eos_reached) break;
    }
    return true;
}

static void *client_thread(void *arg) {
    struct Session s;
    memset(&s, 0, sizeof(s));
    s.sock = (int)(intptr_t)arg;
    s.shm_fd = -1;

    uint32_t hdr[2];
    while (read_all(s.sock, hdr, sizeof(hdr))) {
        uint32_t kind = hdr[0];
        uint32_t len = hdr[1];
        if (len > MAX_PACKET_BYTES) break;
        uint8_t *buf = len > 0 ? malloc(len) : NULL;
        if (len > 0 && (!buf || !read_all(s.sock, buf, len))) {
            free(buf);
            break;
        }
        if (kind == 1 && len >= 20 && !s.codec) {
            uint32_t codec_id, w, h, csd0_len, csd1_len;
            memcpy(&codec_id, buf + 0, 4);
            memcpy(&w, buf + 4, 4);
            memcpy(&h, buf + 8, 4);
            memcpy(&csd0_len, buf + 12, 4);
            memcpy(&csd1_len, buf + 16, 4);
            const char *mime = codec_mime(codec_id);
            if (!mime || w < 16 || h < 16 || w > 4096 || h > 4096 ||
                20u + csd0_len + csd1_len > len) {
                uint32_t err = 1;
                send_frame_packet(s.sock, 1, &err, 4, NULL, 0);
                free(buf);
                continue;
            }
            AMediaCodec *codec = AMediaCodec_createDecoderByType(mime);
            AImageReader *reader = NULL;
            ANativeWindow *win = NULL;
            AMediaFormat *fmt = AMediaFormat_new();
            media_status_t st = codec ? AMEDIA_OK : AMEDIA_ERROR_UNSUPPORTED;
            if (st == AMEDIA_OK) {
                st = AImageReader_newWithUsage(
                    (int32_t)w, (int32_t)h, AIMAGE_FORMAT_PRIVATE,
                    AHARDWAREBUFFER_USAGE_GPU_SAMPLED_IMAGE | AHARDWAREBUFFER_USAGE_COMPOSER_OVERLAY,
                    8, &reader);
            }
            if (st == AMEDIA_OK) st = AImageReader_getWindow(reader, &win);
            if (st == AMEDIA_OK) {
                AMediaFormat_setString(fmt, AMEDIAFORMAT_KEY_MIME, "video/avc");
                AMediaFormat_setInt32(fmt, AMEDIAFORMAT_KEY_WIDTH, (int32_t)w);
                AMediaFormat_setInt32(fmt, AMEDIAFORMAT_KEY_HEIGHT, (int32_t)h);
                if (csd0_len > 0) AMediaFormat_setBuffer(fmt, "csd-0", buf + 20, csd0_len);
                if (csd1_len > 0) AMediaFormat_setBuffer(fmt, "csd-1", buf + 20 + csd0_len, csd1_len);
                st = AMediaCodec_configure(codec, fmt, win, NULL, 0);
            }
            if (st == AMEDIA_OK) st = AMediaCodec_start(codec);
            AMediaFormat_delete(fmt);
            if (st == AMEDIA_OK) {
                s.codec = codec;
                s.reader = reader;
                s.width = w;
                s.height = h;
                char *name = NULL;
                AMediaCodec_getName(codec, &name);
                const char *cname = name ? name : "unknown";
                size_t nlen = strlen(cname);
                uint8_t *reply = malloc(4 + nlen);
                if (reply) {
                    uint32_t ok_code = 0;
                    memcpy(reply, &ok_code, 4);
                    memcpy(reply + 4, cname, nlen);
                    send_frame_packet(s.sock, 1, reply, (uint32_t)(4 + nlen), NULL, 0);
                    free(reply);
                }
                if (name) AMediaCodec_releaseName(codec, name);
            } else {
                if (codec) AMediaCodec_delete(codec);
                if (reader) AImageReader_delete(reader);
                uint32_t err = (uint32_t)(-st);
                if (err == 0) err = 2;
                send_frame_packet(s.sock, 1, &err, 4, NULL, 0);
            }
        } else if (kind == 2 && len >= 12 && s.codec) {
            int64_t pts_us;
            uint32_t flags;
            memcpy(&pts_us, buf + 0, 8);
            memcpy(&flags, buf + 8, 4);
            uint32_t pkt_len = len - 12;
            uint32_t emitted = 0;
            uint32_t eos_reached = 0;
            bool queued = false;
            for (int tries = 0; tries < 50 && !queued; tries++) {
                ssize_t idx = AMediaCodec_dequeueInputBuffer(s.codec, 10000);
                if (idx >= 0) {
                    size_t bsz = 0;
                    uint8_t *ib = AMediaCodec_getInputBuffer(s.codec, idx, &bsz);
                    if (ib && bsz >= pkt_len) {
                        if (pkt_len > 0) memcpy(ib, buf + 12, pkt_len);
                        uint32_t mc_flags = 0;
                        if (flags & 1) mc_flags |= AMEDIACODEC_BUFFER_FLAG_END_OF_STREAM;
                        if (flags & 2) mc_flags |= AMEDIACODEC_BUFFER_FLAG_CODEC_CONFIG;
                        AMediaCodec_queueInputBuffer(s.codec, idx, 0, pkt_len, (uint64_t)pts_us, mc_flags);
                        queued = true;
                    }
                } else {
                    if (!drain_outputs(&s, &emitted, &eos_reached, 5000)) break;
                }
            }
            if (flags & 1) {
                for (int w = 0; w < 60 && !eos_reached; w++) {
                    if (!drain_outputs(&s, &emitted, &eos_reached, 15000)) break;
                }
            } else {
                drain_outputs(&s, &emitted, &eos_reached, 2000);
            }
            uint32_t ack[2] = {emitted, eos_reached};
            if (!send_frame_packet(s.sock, 3, ack, sizeof(ack), NULL, 0)) {
                free(buf);
                break;
            }
        } else if (kind == 3 && len == 4) {
            uint32_t fid;
            memcpy(&fid, buf, 4);
            for (size_t i = 0; i < MAX_SLOTS; i++) {
                if (s.slots[i].used && s.slots[i].frame_id == fid) {
                    release_slot(&s.slots[i]);
                    break;
                }
            }
        } else if (kind == 4 && s.codec) {
            for (size_t i = 0; i < MAX_SLOTS; i++) release_slot(&s.slots[i]);
            AMediaCodec_flush(s.codec);
        } else if (kind == 10 && len >= 20 && !s.codec) {
            uint32_t codec_id, w, h, csd0_len, csd1_len;
            memcpy(&codec_id, buf + 0, 4);
            memcpy(&w, buf + 4, 4);
            memcpy(&h, buf + 8, 4);
            memcpy(&csd0_len, buf + 12, 4);
            memcpy(&csd1_len, buf + 16, 4);
            const char *mime = codec_mime(codec_id);
            if (!mime || w < 16 || h < 16 || w > 4096 || h > 4096 ||
                20u + csd0_len + csd1_len > len) {
                uint32_t err = 1;
                send_frame_packet(s.sock, 1, &err, 4, NULL, 0);
                free(buf);
                continue;
            }
            size_t shm_size = (size_t)YUV_SHM_SLOTS * (size_t)YUV_SLOT_BYTES;
            int shm_fd = (int)syscall(__NR_memfd_create, "bareboot-media-yuv", MFD_CLOEXEC);
            uint8_t *shm_ptr = MAP_FAILED;
            if (shm_fd >= 0 && ftruncate(shm_fd, (off_t)shm_size) == 0) {
                shm_ptr = mmap(NULL, shm_size, PROT_READ | PROT_WRITE, MAP_SHARED, shm_fd, 0);
            }
            AMediaCodec *codec = shm_ptr != MAP_FAILED ? AMediaCodec_createDecoderByType(mime) : NULL;
            AImageReader *reader = NULL;
            ANativeWindow *win = NULL;
            AMediaFormat *fmt = AMediaFormat_new();
            media_status_t st = codec ? AMEDIA_OK : AMEDIA_ERROR_UNSUPPORTED;
            if (st == AMEDIA_OK) {
                st = AImageReader_newWithUsage(
                    (int32_t)w, (int32_t)h, AIMAGE_FORMAT_YUV_420_888,
                    AHARDWAREBUFFER_USAGE_CPU_READ_OFTEN, 6, &reader);
            }
            if (st == AMEDIA_OK) st = AImageReader_getWindow(reader, &win);
            if (st == AMEDIA_OK) {
                AMediaFormat_setString(fmt, AMEDIAFORMAT_KEY_MIME, "video/avc");
                AMediaFormat_setInt32(fmt, AMEDIAFORMAT_KEY_WIDTH, (int32_t)w);
                AMediaFormat_setInt32(fmt, AMEDIAFORMAT_KEY_HEIGHT, (int32_t)h);
                AMediaFormat_setInt32(fmt, "operating-rate", 120);
                AMediaFormat_setInt32(fmt, "priority", 0);
                AMediaFormat_setInt32(fmt, "low-latency", 1);
                if (csd0_len > 0) AMediaFormat_setBuffer(fmt, "csd-0", buf + 20, csd0_len);
                if (csd1_len > 0) AMediaFormat_setBuffer(fmt, "csd-1", buf + 20 + csd0_len, csd1_len);
                st = AMediaCodec_configure(codec, fmt, win, NULL, 0);
            }
            if (st == AMEDIA_OK) st = AMediaCodec_start(codec);
            AMediaFormat_delete(fmt);
            if (st == AMEDIA_OK) {
                s.codec = codec;
                s.reader = reader;
                s.width = w;
                s.height = h;
                s.yuv_mode = true;
                s.shm_fd = shm_fd;
                s.shm_ptr = shm_ptr;
                s.shm_size = shm_size;
                char *name = NULL;
                AMediaCodec_getName(codec, &name);
                const char *cname = name ? name : "unknown";
                size_t nlen = strlen(cname);
                uint8_t *reply = malloc(4 + nlen);
                if (reply) {
                    uint32_t ok_code = 0;
                    memcpy(reply, &ok_code, 4);
                    memcpy(reply + 4, cname, nlen);
                    send_frame_packet(s.sock, 1, reply, (uint32_t)(4 + nlen), &shm_fd, 1);
                    free(reply);
                }
                if (name) AMediaCodec_releaseName(codec, name);
            } else {
                if (codec) AMediaCodec_delete(codec);
                if (reader) AImageReader_delete(reader);
                if (shm_ptr != MAP_FAILED) munmap(shm_ptr, shm_size);
                if (shm_fd >= 0) close(shm_fd);
                uint32_t err = (uint32_t)(-st);
                if (err == 0) err = 2;
                send_frame_packet(s.sock, 1, &err, 4, NULL, 0);
            }
        } else if (kind == 11 && len >= 12 && s.codec && s.yuv_mode) {
            int64_t pts_us;
            uint32_t flags;
            memcpy(&pts_us, buf + 0, 8);
            memcpy(&flags, buf + 8, 4);
            uint32_t pkt_len = len - 12;
            uint32_t emitted = 0;
            uint32_t eos_reached = 0;
            bool queued = false;
            for (int tries = 0; tries < 50 && !queued; tries++) {
                ssize_t idx = AMediaCodec_dequeueInputBuffer(s.codec, 10000);
                if (idx >= 0) {
                    size_t bsz = 0;
                    uint8_t *ib = AMediaCodec_getInputBuffer(s.codec, idx, &bsz);
                    if (ib && bsz >= pkt_len) {
                        if (pkt_len > 0) memcpy(ib, buf + 12, pkt_len);
                        uint32_t mc_flags = 0;
                        if (flags & 1) mc_flags |= AMEDIACODEC_BUFFER_FLAG_END_OF_STREAM;
                        if (flags & 2) mc_flags |= AMEDIACODEC_BUFFER_FLAG_CODEC_CONFIG;
                        AMediaCodec_queueInputBuffer(s.codec, idx, 0, pkt_len, (uint64_t)pts_us, mc_flags);
                        queued = true;
                    }
                } else {
                    if (!drain_outputs_yuv(&s, &emitted, &eos_reached, 5000)) break;
                }
            }
            if (flags & 1) {
                for (int w = 0; w < 60 && !eos_reached; w++) {
                    if (!drain_outputs_yuv(&s, &emitted, &eos_reached, 15000)) break;
                }
            } else {
                drain_outputs_yuv(&s, &emitted, &eos_reached, 500);
            }
            uint32_t ack[2] = {emitted, eos_reached};
            if (!send_frame_packet(s.sock, 3, ack, sizeof(ack), NULL, 0)) {
                free(buf);
                break;
            }
        }
        free(buf);
    }

    session_cleanup(&s);
    close(s.sock);
    return NULL;
}

int main(void) {
    const char *parent_env = getenv("BAREBOOT_PARENT_PID");
    const char *sock_path = getenv("BAREBOOT_MEDIA_SOCKET");
    const char *ahb_env = getenv("BAREBOOT_MEDIA_AHB_FD");
    const char *enc_env = getenv("BAREBOOT_ENC_FD");
    if (!parent_env || !sock_path || !ahb_env) return 2;
    pid_t parent = (pid_t)atoi(parent_env);
    if (parent <= 1 || prctl(PR_SET_PDEATHSIG, SIGKILL) != 0 || getppid() != parent) return 2;
    g_ahb_fd = atoi(ahb_env);
    if (g_ahb_fd <= 2) return 2;

    void *bn = dlopen("libbinder_ndk.so", RTLD_NOW);
    if (bn) {
        void (*start_tp)(void) = (void (*)(void))dlsym(bn, "ABinderProcess_startThreadPool");
        if (start_tp) start_tp();
    }

    if (enc_env) {
        int enc_fd = atoi(enc_env);
        if (enc_fd > 2) {
            pthread_t eth;
            if (pthread_create(&eth, NULL, encoder_thread, (void *)(intptr_t)enc_fd) == 0) {
                pthread_detach(eth);
            }
        }
    }

    int listen_fd = socket(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0);
    if (listen_fd < 0) return 1;
    struct sockaddr_un addr;
    memset(&addr, 0, sizeof(addr));
    addr.sun_family = AF_UNIX;
    if (strlen(sock_path) >= sizeof(addr.sun_path)) return 1;
    strcpy(addr.sun_path, sock_path);
    unlink(sock_path);
    if (bind(listen_fd, (struct sockaddr *)&addr, sizeof(addr)) != 0) return 1;
    chmod(sock_path, 0660);
    if (listen(listen_fd, 8) != 0) return 1;

    for (;;) {
        int cfd = accept4(listen_fd, NULL, NULL, SOCK_CLOEXEC);
        if (cfd < 0) {
            if (errno == EINTR) continue;
            break;
        }
        pthread_t th;
        if (pthread_create(&th, NULL, client_thread, (void *)(intptr_t)cfd) == 0) {
            pthread_detach(th);
        } else {
            close(cfd);
        }
    }
    close(listen_fd);
    return 0;
}
