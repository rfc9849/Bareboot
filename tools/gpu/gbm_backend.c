#include <errno.h>
#include <fcntl.h>
#include <stdbool.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <vulkan/vulkan.h>

#include "gbm_backend_abi.h"

#define FOURCC(a, b, c, d) ((uint32_t)(a) | ((uint32_t)(b) << 8) | ((uint32_t)(c) << 16) | ((uint32_t)(d) << 24))
#define FMT_ARGB8888 FOURCC('A', 'R', '2', '4')
#define FMT_XRGB8888 FOURCC('X', 'R', '2', '4')
#define FMT_ABGR8888 FOURCC('A', 'B', '2', '4')
#define FMT_XBGR8888 FOURCC('X', 'B', '2', '4')
#define MOD_LINEAR 0ull
#define MOD_INVALID 0x00ffffffffffffffull

struct device {
   struct gbm_device base;
   VkInstance instance;
   VkDevice device;
   PFN_vkGetMemoryFdKHR get_memory_fd;
};

struct bo {
   struct gbm_bo base;
   int fds[4];
   uint32_t strides[4];
   uint32_t offsets[4];
   int planes;
   VkImage image;
   VkDeviceMemory memory;
};

static const struct gbm_core *core;

static uint32_t
canonical(uint32_t format)
{
   return core->v0.format_canonicalize ? core->v0.format_canonicalize(format) : format;
}

static VkFormat
vk_format(uint32_t format)
{
   switch (canonical(format)) {
   case FMT_ABGR8888:
   case FMT_XBGR8888:
      return VK_FORMAT_R8G8B8A8_UNORM;
   case FMT_ARGB8888:
   case FMT_XRGB8888:
      return VK_FORMAT_B8G8R8A8_UNORM;
   default:
      return VK_FORMAT_UNDEFINED;
   }
}

static int
is_format_supported(struct gbm_device *gbm, uint32_t format, uint32_t usage)
{
   (void)gbm;
   if (usage & (GBM_BO_USE_CURSOR | GBM_BO_USE_WRITE | GBM_BO_USE_PROTECTED))
      return 0;
   return vk_format(format) != VK_FORMAT_UNDEFINED;
}

static int
get_format_modifier_plane_count(struct gbm_device *gbm, uint32_t format, uint64_t modifier)
{
   (void)gbm;
   if (vk_format(format) == VK_FORMAT_UNDEFINED || modifier != MOD_LINEAR)
      return -1;
   return 1;
}

static void
bo_free(struct bo *bo)
{
   struct device *dev = (struct device *)bo->base.gbm;
   for (int i = 0; i < bo->planes; i++) {
      if (bo->fds[i] >= 0)
         close(bo->fds[i]);
   }
   if (bo->image)
      vkDestroyImage(dev->device, bo->image, NULL);
   if (bo->memory)
      vkFreeMemory(dev->device, bo->memory, NULL);
   free(bo);
}

static struct bo *
bo_new(struct gbm_device *gbm, uint32_t width, uint32_t height, uint32_t format)
{
   struct bo *bo = calloc(1, sizeof(*bo));
   if (!bo) {
      errno = ENOMEM;
      return NULL;
   }
   bo->base.gbm = gbm;
   bo->base.v0.width = width;
   bo->base.v0.height = height;
   bo->base.v0.format = format;
   for (int i = 0; i < 4; i++)
      bo->fds[i] = -1;
   return bo;
}

static struct gbm_bo *
bo_create(struct gbm_device *gbm, uint32_t width, uint32_t height, uint32_t format,
          uint32_t usage, const uint64_t *modifiers, const unsigned int count)
{
   struct device *dev = (struct device *)gbm;
   VkFormat vkf = vk_format(format);
   bool linear = count == 0;
   for (unsigned int i = 0; i < count; i++)
      linear |= modifiers[i] == MOD_LINEAR || modifiers[i] == MOD_INVALID;
   if (vkf == VK_FORMAT_UNDEFINED || !linear || !width || !height ||
       (usage & (GBM_BO_USE_CURSOR | GBM_BO_USE_WRITE | GBM_BO_USE_PROTECTED))) {
      errno = EINVAL;
      return NULL;
   }
   struct bo *bo = bo_new(gbm, width, height, format);
   if (!bo)
      return NULL;

   uint64_t modifier = MOD_LINEAR;
   VkImageDrmFormatModifierListCreateInfoEXT list = {
      .sType = VK_STRUCTURE_TYPE_IMAGE_DRM_FORMAT_MODIFIER_LIST_CREATE_INFO_EXT,
      .drmFormatModifierCount = 1,
      .pDrmFormatModifiers = &modifier,
   };
   VkExternalMemoryImageCreateInfo external = {
      .sType = VK_STRUCTURE_TYPE_EXTERNAL_MEMORY_IMAGE_CREATE_INFO,
      .pNext = &list,
      .handleTypes = VK_EXTERNAL_MEMORY_HANDLE_TYPE_DMA_BUF_BIT_EXT,
   };
   VkImageCreateInfo image_info = {
      .sType = VK_STRUCTURE_TYPE_IMAGE_CREATE_INFO,
      .pNext = &external,
      .imageType = VK_IMAGE_TYPE_2D,
      .format = vkf,
      .extent = {width, height, 1},
      .mipLevels = 1,
      .arrayLayers = 1,
      .samples = VK_SAMPLE_COUNT_1_BIT,
      .tiling = VK_IMAGE_TILING_DRM_FORMAT_MODIFIER_EXT,
      .usage = VK_IMAGE_USAGE_COLOR_ATTACHMENT_BIT | VK_IMAGE_USAGE_SAMPLED_BIT |
               VK_IMAGE_USAGE_TRANSFER_SRC_BIT | VK_IMAGE_USAGE_TRANSFER_DST_BIT,
      .initialLayout = VK_IMAGE_LAYOUT_UNDEFINED,
   };
   if (vkCreateImage(dev->device, &image_info, NULL, &bo->image) != VK_SUCCESS)
      goto fail;

   VkMemoryRequirements req;
   vkGetImageMemoryRequirements(dev->device, bo->image, &req);
   if (!req.memoryTypeBits)
      goto fail;
   VkMemoryDedicatedAllocateInfo dedicated = {
      .sType = VK_STRUCTURE_TYPE_MEMORY_DEDICATED_ALLOCATE_INFO,
      .image = bo->image,
   };
   VkExportMemoryAllocateInfo export_info = {
      .sType = VK_STRUCTURE_TYPE_EXPORT_MEMORY_ALLOCATE_INFO,
      .pNext = &dedicated,
      .handleTypes = VK_EXTERNAL_MEMORY_HANDLE_TYPE_DMA_BUF_BIT_EXT,
   };
   VkMemoryAllocateInfo alloc = {
      .sType = VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO,
      .pNext = &export_info,
      .allocationSize = req.size,
      .memoryTypeIndex = (uint32_t)__builtin_ctz(req.memoryTypeBits),
   };
   if (vkAllocateMemory(dev->device, &alloc, NULL, &bo->memory) != VK_SUCCESS ||
       vkBindImageMemory(dev->device, bo->image, bo->memory, 0) != VK_SUCCESS)
      goto fail;

   VkImageSubresource sub = {.aspectMask = VK_IMAGE_ASPECT_MEMORY_PLANE_0_BIT_EXT};
   VkSubresourceLayout layout;
   vkGetImageSubresourceLayout(dev->device, bo->image, &sub, &layout);
   VkMemoryGetFdInfoKHR fd_info = {
      .sType = VK_STRUCTURE_TYPE_MEMORY_GET_FD_INFO_KHR,
      .memory = bo->memory,
      .handleType = VK_EXTERNAL_MEMORY_HANDLE_TYPE_DMA_BUF_BIT_EXT,
   };
   bo->planes = 1;
   if (dev->get_memory_fd(dev->device, &fd_info, &bo->fds[0]) != VK_SUCCESS)
      goto fail;
   bo->strides[0] = (uint32_t)layout.rowPitch;
   bo->offsets[0] = (uint32_t)layout.offset;
   bo->base.v0.stride = bo->strides[0];
   return &bo->base;

fail:
   bo_free(bo);
   errno = ENOMEM;
   return NULL;
}

static struct gbm_bo *
bo_import(struct gbm_device *gbm, uint32_t type, void *buffer, uint32_t usage)
{
   (void)usage;
   struct bo *bo;
   if (type == GBM_BO_IMPORT_FD) {
      struct gbm_import_fd_data *data = buffer;
      if (!(bo = bo_new(gbm, data->width, data->height, data->format)))
         return NULL;
      bo->planes = 1;
      bo->fds[0] = fcntl(data->fd, F_DUPFD_CLOEXEC, 0);
      bo->strides[0] = data->stride;
   } else if (type == GBM_BO_IMPORT_FD_MODIFIER) {
      struct gbm_import_fd_modifier_data *data = buffer;
      if (data->num_fds < 1 || data->num_fds > 4 || data->modifier != MOD_LINEAR ||
          !(bo = bo_new(gbm, data->width, data->height, data->format))) {
         errno = EINVAL;
         return NULL;
      }
      bo->planes = (int)data->num_fds;
      for (int i = 0; i < bo->planes; i++) {
         bo->fds[i] = fcntl(data->fds[i], F_DUPFD_CLOEXEC, 0);
         bo->strides[i] = (uint32_t)data->strides[i];
         bo->offsets[i] = (uint32_t)data->offsets[i];
      }
   } else {
      errno = ENOSYS;
      return NULL;
   }
   for (int i = 0; i < bo->planes; i++) {
      if (bo->fds[i] < 0) {
         bo_free(bo);
         return NULL;
      }
   }
   bo->base.v0.stride = bo->strides[0];
   return &bo->base;
}

static void *
bo_map(struct gbm_bo *bo, uint32_t x, uint32_t y, uint32_t width, uint32_t height,
       uint32_t flags, uint32_t *stride, void **map_data)
{
   (void)bo, (void)x, (void)y, (void)width, (void)height, (void)flags, (void)stride, (void)map_data;
   errno = ENOSYS;
   return NULL;
}

static void
bo_unmap(struct gbm_bo *bo, void *map_data)
{
   (void)bo, (void)map_data;
}

static int
bo_write(struct gbm_bo *bo, const void *buf, size_t data)
{
   (void)bo, (void)buf, (void)data;
   errno = ENOSYS;
   return -1;
}

static int
bo_get_plane_fd(struct gbm_bo *gbo, int plane)
{
   struct bo *bo = (struct bo *)gbo;
   if (plane < 0 || plane >= bo->planes)
      return -1;
   return fcntl(bo->fds[plane], F_DUPFD_CLOEXEC, 0);
}

static int
bo_get_fd(struct gbm_bo *gbo)
{
   return bo_get_plane_fd(gbo, 0);
}

static int
bo_get_planes(struct gbm_bo *gbo)
{
   return ((struct bo *)gbo)->planes;
}

static union gbm_bo_handle
bo_get_handle(struct gbm_bo *gbo, int plane)
{
   (void)gbo, (void)plane;
   union gbm_bo_handle handle = {.u64 = 0};
   return handle;
}

static uint32_t
bo_get_stride(struct gbm_bo *gbo, int plane)
{
   struct bo *bo = (struct bo *)gbo;
   return plane >= 0 && plane < bo->planes ? bo->strides[plane] : 0;
}

static uint32_t
bo_get_offset(struct gbm_bo *gbo, int plane)
{
   struct bo *bo = (struct bo *)gbo;
   return plane >= 0 && plane < bo->planes ? bo->offsets[plane] : 0;
}

static uint64_t
bo_get_modifier(struct gbm_bo *gbo)
{
   (void)gbo;
   return MOD_LINEAR;
}

static void
bo_destroy(struct gbm_bo *gbo)
{
   bo_free((struct bo *)gbo);
}

static struct gbm_surface *
surface_create(struct gbm_device *gbm, uint32_t width, uint32_t height, uint32_t format,
               uint32_t flags, const uint64_t *modifiers, const unsigned count)
{
   (void)gbm, (void)width, (void)height, (void)format, (void)flags, (void)modifiers, (void)count;
   errno = ENOSYS;
   return NULL;
}

static struct gbm_bo *
surface_lock_front_buffer(struct gbm_surface *surface)
{
   (void)surface;
   return NULL;
}

static void
surface_release_buffer(struct gbm_surface *surface, struct gbm_bo *bo)
{
   (void)surface, (void)bo;
}

static int
surface_has_free_buffers(struct gbm_surface *surface)
{
   (void)surface;
   return 0;
}

static void
surface_destroy(struct gbm_surface *surface)
{
   (void)surface;
}

static void
device_destroy(struct gbm_device *gbm)
{
   struct device *dev = (struct device *)gbm;
   if (dev->device)
      vkDestroyDevice(dev->device, NULL);
   if (dev->instance)
      vkDestroyInstance(dev->instance, NULL);
   free(dev);
}

static bool
vulkan_init(struct device *dev)
{
   VkApplicationInfo app = {
      .sType = VK_STRUCTURE_TYPE_APPLICATION_INFO,
      .pApplicationName = "bareboot-gbm",
      .apiVersion = VK_API_VERSION_1_1,
   };
   VkInstanceCreateInfo instance_info = {
      .sType = VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO,
      .pApplicationInfo = &app,
   };
   if (vkCreateInstance(&instance_info, NULL, &dev->instance) != VK_SUCCESS)
      return false;
   uint32_t count = 1;
   VkPhysicalDevice physical;
   VkResult res = vkEnumeratePhysicalDevices(dev->instance, &count, &physical);
   if ((res != VK_SUCCESS && res != VK_INCOMPLETE) || count == 0)
      return false;
   float priority = 1.0f;
   VkDeviceQueueCreateInfo queue = {
      .sType = VK_STRUCTURE_TYPE_DEVICE_QUEUE_CREATE_INFO,
      .queueCount = 1,
      .pQueuePriorities = &priority,
   };
   const char *extensions[] = {
      "VK_KHR_external_memory_fd",
      "VK_EXT_external_memory_dma_buf",
      "VK_EXT_image_drm_format_modifier",
      "VK_KHR_image_format_list",
   };
   VkDeviceCreateInfo device_info = {
      .sType = VK_STRUCTURE_TYPE_DEVICE_CREATE_INFO,
      .queueCreateInfoCount = 1,
      .pQueueCreateInfos = &queue,
      .enabledExtensionCount = sizeof(extensions) / sizeof(extensions[0]),
      .ppEnabledExtensionNames = extensions,
   };
   if (vkCreateDevice(physical, &device_info, NULL, &dev->device) != VK_SUCCESS)
      return false;
   dev->get_memory_fd = (PFN_vkGetMemoryFdKHR)vkGetDeviceProcAddr(dev->device, "vkGetMemoryFdKHR");
   return dev->get_memory_fd != NULL;
}

static struct gbm_device *
create_device(int fd, uint32_t gbm_backend_version)
{
   struct device *dev = calloc(1, sizeof(*dev));
   if (!dev)
      return NULL;
   dev->base.v0.backend_version = gbm_backend_version;
   dev->base.v0.fd = fd;
   dev->base.v0.name = "bareboot";
   dev->base.v0.destroy = device_destroy;
   dev->base.v0.is_format_supported = is_format_supported;
   dev->base.v0.get_format_modifier_plane_count = get_format_modifier_plane_count;
   dev->base.v0.bo_create = bo_create;
   dev->base.v0.bo_import = bo_import;
   dev->base.v0.bo_map = bo_map;
   dev->base.v0.bo_unmap = bo_unmap;
   dev->base.v0.bo_write = bo_write;
   dev->base.v0.bo_get_fd = bo_get_fd;
   dev->base.v0.bo_get_planes = bo_get_planes;
   dev->base.v0.bo_get_handle = bo_get_handle;
   dev->base.v0.bo_get_plane_fd = bo_get_plane_fd;
   dev->base.v0.bo_get_stride = bo_get_stride;
   dev->base.v0.bo_get_offset = bo_get_offset;
   dev->base.v0.bo_get_modifier = bo_get_modifier;
   dev->base.v0.bo_destroy = bo_destroy;
   dev->base.v0.surface_create = surface_create;
   dev->base.v0.surface_lock_front_buffer = surface_lock_front_buffer;
   dev->base.v0.surface_release_buffer = surface_release_buffer;
   dev->base.v0.surface_has_free_buffers = surface_has_free_buffers;
   dev->base.v0.surface_destroy = surface_destroy;
   if (!vulkan_init(dev)) {
      device_destroy(&dev->base);
      return NULL;
   }
   return &dev->base;
}

static const struct gbm_backend backend = {
   .v0 = {
      .backend_version = 1,
      .backend_name = "bareboot",
      .create_device = create_device,
   },
};

__attribute__((visibility("default"))) const struct gbm_backend *
gbmint_get_backend(const struct gbm_core *gbm_core)
{
   core = gbm_core;
   return &backend;
}
