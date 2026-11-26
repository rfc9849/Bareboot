#include <vulkan/vulkan.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#define CHECK(x) do { VkResult r_ = (x); if (r_ != VK_SUCCESS) { fprintf(stderr, "%s failed: %d\n", #x, r_); return 1; } } while (0)
static double now(void) { struct timespec t; clock_gettime(CLOCK_MONOTONIC, &t); return t.tv_sec * 1e3 + t.tv_nsec / 1e6; }
int main(int argc, char **argv) {
    const uint32_t N = 1 << 20;
    if (argc != 2) return 1;
    FILE *f = fopen(argv[1], "rb");
    if (!f) { perror(argv[1]); return 1; }
    if (fseek(f, 0, SEEK_END)) return 1;
    long sz = ftell(f);
    if (sz <= 0 || sz % 4 || fseek(f, 0, SEEK_SET)) return 1;
    uint32_t *code = malloc(sz);
    if (!code || fread(code, 1, sz, f) != (size_t)sz) return 1;
    fclose(f);
    double t0 = now();
    VkApplicationInfo app = { VK_STRUCTURE_TYPE_APPLICATION_INFO, 0, "compute", 1, 0, 0, VK_API_VERSION_1_1 };
    VkInstanceCreateInfo ici = { VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO, 0, 0, &app };
    VkInstance inst; CHECK(vkCreateInstance(&ici, 0, &inst));
    uint32_t n = 1;
    VkPhysicalDevice pd;
    VkResult devices = vkEnumeratePhysicalDevices(inst, &n, &pd);
    if ((devices != VK_SUCCESS && devices != VK_INCOMPLETE) || !n) {
        fprintf(stderr, "No Vulkan device\n");
        return 1;
    }
    VkPhysicalDeviceProperties props;
    vkGetPhysicalDeviceProperties(pd, &props);
    const char prefix[] = "Virtio-GPU GFXStream (";
    if (strncmp(props.deviceName, prefix, sizeof(prefix) - 1) != 0) {
        fprintf(stderr, "Vulkan selected %s instead of Gfxstream\n", props.deviceName);
        return 1;
    }
    printf("device: %s\n", props.deviceName);
    float prio = 1; VkDeviceQueueCreateInfo qci = { VK_STRUCTURE_TYPE_DEVICE_QUEUE_CREATE_INFO, 0, 0, 0, 1, &prio };
    uint32_t qn = 0; vkGetPhysicalDeviceQueueFamilyProperties(pd, &qn, 0);
    VkQueueFamilyProperties *qp = calloc(qn, sizeof(*qp));
    if (!qn || !qp) return 1;
    vkGetPhysicalDeviceQueueFamilyProperties(pd, &qn, qp);
    uint32_t family = 0;
    while (family < qn && !(qp[family].queueFlags & VK_QUEUE_COMPUTE_BIT)) family++;
    free(qp);
    if (family == qn) return 1;
    qci.queueFamilyIndex = family;
    VkDeviceCreateInfo dci = { VK_STRUCTURE_TYPE_DEVICE_CREATE_INFO, 0, 0, 1, &qci };
    VkDevice dev; CHECK(vkCreateDevice(pd, &dci, 0, &dev));
    VkQueue q; vkGetDeviceQueue(dev, qci.queueFamilyIndex, 0, &q);
    VkBufferCreateInfo bci = { VK_STRUCTURE_TYPE_BUFFER_CREATE_INFO, 0, 0, N * 4, VK_BUFFER_USAGE_STORAGE_BUFFER_BIT };
    VkBuffer buf; CHECK(vkCreateBuffer(dev, &bci, 0, &buf));
    VkMemoryRequirements mr; vkGetBufferMemoryRequirements(dev, buf, &mr);
    VkPhysicalDeviceMemoryProperties mp; vkGetPhysicalDeviceMemoryProperties(pd, &mp);
    uint32_t mt = UINT32_MAX;
    for (uint32_t i = 0; i < mp.memoryTypeCount; i++)
        if ((mr.memoryTypeBits & (1u << i)) && (mp.memoryTypes[i].propertyFlags & (VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT | VK_MEMORY_PROPERTY_HOST_COHERENT_BIT)) == (VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT | VK_MEMORY_PROPERTY_HOST_COHERENT_BIT)) { mt = i; break; }
    if (mt == UINT32_MAX) { fprintf(stderr, "no host-visible coherent memory\n"); return 1; }
    VkMemoryAllocateInfo mai = { VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO, 0, mr.size, mt };
    VkDeviceMemory mem; CHECK(vkAllocateMemory(dev, &mai, 0, &mem)); CHECK(vkBindBufferMemory(dev, buf, mem, 0));
    uint32_t *p; CHECK(vkMapMemory(dev, mem, 0, N * 4, 0, (void **)&p));
    for (uint32_t i = 0; i < N; i++) p[i] = i;
    VkShaderModuleCreateInfo smci = { VK_STRUCTURE_TYPE_SHADER_MODULE_CREATE_INFO, 0, 0, sz, code };
    VkShaderModule sm; CHECK(vkCreateShaderModule(dev, &smci, 0, &sm));
    free(code);
    VkDescriptorSetLayoutBinding b = { 0, VK_DESCRIPTOR_TYPE_STORAGE_BUFFER, 1, VK_SHADER_STAGE_COMPUTE_BIT };
    VkDescriptorSetLayoutCreateInfo dslci = { VK_STRUCTURE_TYPE_DESCRIPTOR_SET_LAYOUT_CREATE_INFO, 0, 0, 1, &b };
    VkDescriptorSetLayout dsl; CHECK(vkCreateDescriptorSetLayout(dev, &dslci, 0, &dsl));
    VkPipelineLayoutCreateInfo plci = { VK_STRUCTURE_TYPE_PIPELINE_LAYOUT_CREATE_INFO, 0, 0, 1, &dsl };
    VkPipelineLayout pl; CHECK(vkCreatePipelineLayout(dev, &plci, 0, &pl));
    VkComputePipelineCreateInfo cpci = { VK_STRUCTURE_TYPE_COMPUTE_PIPELINE_CREATE_INFO, 0, 0,
        { VK_STRUCTURE_TYPE_PIPELINE_SHADER_STAGE_CREATE_INFO, 0, 0, VK_SHADER_STAGE_COMPUTE_BIT, sm, "main" }, pl };
    VkPipeline pipe; CHECK(vkCreateComputePipelines(dev, 0, 1, &cpci, 0, &pipe));
    VkDescriptorPoolSize ps = { VK_DESCRIPTOR_TYPE_STORAGE_BUFFER, 1 };
    VkDescriptorPoolCreateInfo dpci = { VK_STRUCTURE_TYPE_DESCRIPTOR_POOL_CREATE_INFO, 0, 0, 1, 1, &ps };
    VkDescriptorPool dp; CHECK(vkCreateDescriptorPool(dev, &dpci, 0, &dp));
    VkDescriptorSetAllocateInfo dsai = { VK_STRUCTURE_TYPE_DESCRIPTOR_SET_ALLOCATE_INFO, 0, dp, 1, &dsl };
    VkDescriptorSet ds; CHECK(vkAllocateDescriptorSets(dev, &dsai, &ds));
    VkDescriptorBufferInfo dbi = { buf, 0, VK_WHOLE_SIZE };
    VkWriteDescriptorSet w = { VK_STRUCTURE_TYPE_WRITE_DESCRIPTOR_SET, 0, ds, 0, 0, 1, VK_DESCRIPTOR_TYPE_STORAGE_BUFFER, 0, &dbi };
    vkUpdateDescriptorSets(dev, 1, &w, 0, 0);
    VkCommandPoolCreateInfo cpi = { VK_STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO, 0, 0, qci.queueFamilyIndex };
    VkCommandPool cp; CHECK(vkCreateCommandPool(dev, &cpi, 0, &cp));
    VkCommandBufferAllocateInfo cbai = { VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO, 0, cp, VK_COMMAND_BUFFER_LEVEL_PRIMARY, 1 };
    VkCommandBuffer cb; CHECK(vkAllocateCommandBuffers(dev, &cbai, &cb));
    VkCommandBufferBeginInfo cbbi = { VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO };
    CHECK(vkBeginCommandBuffer(cb, &cbbi));
    vkCmdBindPipeline(cb, VK_PIPELINE_BIND_POINT_COMPUTE, pipe);
    vkCmdBindDescriptorSets(cb, VK_PIPELINE_BIND_POINT_COMPUTE, pl, 0, 1, &ds, 0, 0);
    vkCmdDispatch(cb, N / 64, 1, 1);
    VkMemoryBarrier mb = { VK_STRUCTURE_TYPE_MEMORY_BARRIER, 0, VK_ACCESS_SHADER_WRITE_BIT, VK_ACCESS_HOST_READ_BIT };
    vkCmdPipelineBarrier(cb, VK_PIPELINE_STAGE_COMPUTE_SHADER_BIT, VK_PIPELINE_STAGE_HOST_BIT, 0, 1, &mb, 0, 0, 0, 0);
    CHECK(vkEndCommandBuffer(cb));
    VkFenceCreateInfo fci = { VK_STRUCTURE_TYPE_FENCE_CREATE_INFO };
    VkFence fence; CHECK(vkCreateFence(dev, &fci, 0, &fence));
    double t1 = now();
    VkSubmitInfo si = { VK_STRUCTURE_TYPE_SUBMIT_INFO, 0, 0, 0, 0, 1, &cb };
    CHECK(vkQueueSubmit(q, 1, &si, fence));
    CHECK(vkWaitForFences(dev, 1, &fence, VK_TRUE, 10000000000ull));
    double t2 = now();
    uint32_t bad = 0, stale = 0, first = UINT32_MAX, last = 0;
    for (uint32_t i = 0; i < N; i++) if (p[i] != i * 3u + 7u) { bad++; if (p[i] == i) stale++; if (first == UINT32_MAX) first = i; last = i; }
    if (bad) printf("first_bad=%u last_bad=%u stale_input=%u sample=%u expected=%u\n", first, last, stale, p[first], first * 3u + 7u);
    printf("words=%u bad=%u setup_ms=%.2f submit_wait_ms=%.2f total_ms=%.2f\n", N, bad, t1 - t0, t2 - t1, now() - t0);
    return bad != 0;
}
