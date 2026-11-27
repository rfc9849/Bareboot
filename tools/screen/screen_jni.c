#include <android/hardware_buffer.h>
#include <android/hardware_buffer_jni.h>
#include <errno.h>
#include <jni.h>
#include <string.h>
#include <sys/socket.h>
#include <unistd.h>

#define MAX_FDS 16

JNIEXPORT jint JNICALL
Java_ch_ausfuhrli_bareboot_screen_Native_recv(JNIEnv *env, jclass cls, jint fd, jbyteArray buffer, jintArray fds) {
    (void)cls;
    jsize capacity = (*env)->GetArrayLength(env, buffer);
    jsize slots = (*env)->GetArrayLength(env, fds);
    if (slots < 1) return -EINVAL;
    jbyte *data = (*env)->GetByteArrayElements(env, buffer, NULL);
    if (!data) return -ENOMEM;
    union {
        struct cmsghdr align;
        unsigned char data[CMSG_SPACE(MAX_FDS * sizeof(int))];
    } control;
    struct iovec iov = {.iov_base = data, .iov_len = (size_t)capacity};
    struct msghdr msg = {.msg_iov = &iov, .msg_iovlen = 1, .msg_control = &control, .msg_controllen = sizeof(control)};
    ssize_t n;
    do {
        n = recvmsg(fd, &msg, MSG_CMSG_CLOEXEC);
    } while (n < 0 && errno == EINTR);
    int error = errno;
    (*env)->ReleaseByteArrayElements(env, buffer, data, n > 0 ? 0 : JNI_ABORT);
    jint received[MAX_FDS + 1];
    jint count = 0;
    if (n >= 0) {
        for (struct cmsghdr *head = CMSG_FIRSTHDR(&msg); head; head = CMSG_NXTHDR(&msg, head)) {
            if (head->cmsg_level != SOL_SOCKET || head->cmsg_type != SCM_RIGHTS) continue;
            size_t total = (head->cmsg_len - CMSG_LEN(0)) / sizeof(int);
            for (size_t i = 0; i < total; i++) {
                int received_fd;
                memcpy(&received_fd, CMSG_DATA(head) + i * sizeof(int), sizeof(int));
                if (count < MAX_FDS && count + 1 < slots) {
                    received[++count] = received_fd;
                } else {
                    close(received_fd);
                }
            }
        }
    }
    received[0] = count;
    (*env)->SetIntArrayRegion(env, fds, 0, count + 1, received);
    return n < 0 ? -error : (jint)n;
}

JNIEXPORT jobject JNICALL
Java_ch_ausfuhrli_bareboot_screen_Native_buffer(JNIEnv *env, jclass cls, jbyteArray metadata, jintArray fds) {
    (void)cls;
    jsize length = (*env)->GetArrayLength(env, metadata);
    jsize count = (*env)->GetArrayLength(env, fds);
    int handles[MAX_FDS];
    if (count > MAX_FDS) count = MAX_FDS;
    (*env)->GetIntArrayRegion(env, fds, 0, count, handles);
    jobject result = NULL;
    int pair[2];
    if (length > 0 && count > 0 && socketpair(AF_UNIX, SOCK_SEQPACKET | SOCK_CLOEXEC, 0, pair) == 0) {
        jbyte *data = (*env)->GetByteArrayElements(env, metadata, NULL);
        union {
            struct cmsghdr align;
            unsigned char data[CMSG_SPACE(MAX_FDS * sizeof(int))];
        } control;
        struct iovec iov = {.iov_base = data, .iov_len = (size_t)length};
        struct msghdr msg = {.msg_iov = &iov, .msg_iovlen = 1, .msg_control = &control,
                             .msg_controllen = CMSG_SPACE(count * sizeof(int))};
        struct cmsghdr *head = CMSG_FIRSTHDR(&msg);
        head->cmsg_level = SOL_SOCKET;
        head->cmsg_type = SCM_RIGHTS;
        head->cmsg_len = CMSG_LEN(count * sizeof(int));
        memcpy(CMSG_DATA(head), handles, count * sizeof(int));
        AHardwareBuffer *ahb = NULL;
        if (data && sendmsg(pair[0], &msg, MSG_NOSIGNAL) == length &&
            AHardwareBuffer_recvHandleFromUnixSocket(pair[1], &ahb) == 0 && ahb) {
            result = AHardwareBuffer_toHardwareBuffer(env, ahb);
            AHardwareBuffer_release(ahb);
        }
        if (data) (*env)->ReleaseByteArrayElements(env, metadata, data, JNI_ABORT);
        close(pair[0]);
        close(pair[1]);
    }
    for (jsize i = 0; i < count; i++) close(handles[i]);
    return result;
}
