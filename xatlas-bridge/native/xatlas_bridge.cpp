#include "xatlas.h"

#include <cstdint>
#include <new>

struct MetrocalkXatlas {
    xatlas::Atlas *atlas;
};

struct MetrocalkXatlasVertex {
    std::int32_t atlas_index;
    std::int32_t chart_index;
    float uv[2];
    std::uint32_t xref;
};

extern "C" {

MetrocalkXatlas *metrocalk_xatlas_create() noexcept {
    try {
        auto *handle = new (std::nothrow) MetrocalkXatlas{xatlas::Create()};
        if (handle == nullptr || handle->atlas == nullptr) {
            delete handle;
            return nullptr;
        }
        return handle;
    } catch (...) {
        return nullptr;
    }
}

void metrocalk_xatlas_destroy(MetrocalkXatlas *handle) noexcept {
    if (handle != nullptr) {
        xatlas::Destroy(handle->atlas);
        delete handle;
    }
}

std::int32_t metrocalk_xatlas_add_mesh(
    MetrocalkXatlas *handle,
    const float *positions,
    const float *normals,
    std::uint32_t vertex_count,
    const std::uint32_t *indices,
    std::uint32_t index_count,
    std::uint32_t mesh_count_hint) noexcept {
    if (handle == nullptr || handle->atlas == nullptr || positions == nullptr || normals == nullptr ||
        indices == nullptr || vertex_count == 0 || index_count == 0) {
        return static_cast<std::int32_t>(xatlas::AddMeshError::Error);
    }
    try {
        xatlas::MeshDecl declaration;
        declaration.vertexPositionData = positions;
        declaration.vertexNormalData = normals;
        declaration.vertexCount = vertex_count;
        declaration.vertexPositionStride = sizeof(float) * 3;
        declaration.vertexNormalStride = sizeof(float) * 3;
        declaration.indexData = indices;
        declaration.indexCount = index_count;
        declaration.faceCount = index_count / 3;
        declaration.indexFormat = xatlas::IndexFormat::UInt32;
        return static_cast<std::int32_t>(xatlas::AddMesh(handle->atlas, declaration, mesh_count_hint));
    } catch (...) {
        return static_cast<std::int32_t>(xatlas::AddMeshError::Error);
    }
}

void metrocalk_xatlas_generate(
    MetrocalkXatlas *handle,
    float normal_seam_weight,
    std::uint32_t max_iterations,
    std::uint8_t fix_winding,
    std::uint32_t padding,
    float texels_per_unit,
    std::uint32_t resolution,
    std::uint8_t block_align,
    std::uint8_t brute_force) noexcept {
    if (handle == nullptr || handle->atlas == nullptr) {
        return;
    }
    try {
        xatlas::ChartOptions chart_options;
        chart_options.normalSeamWeight = normal_seam_weight;
        chart_options.maxIterations = max_iterations;
        chart_options.fixWinding = fix_winding != 0;

        xatlas::PackOptions pack_options;
        pack_options.padding = padding;
        pack_options.texelsPerUnit = texels_per_unit;
        pack_options.resolution = resolution;
        pack_options.bilinear = true;
        pack_options.blockAlign = block_align != 0;
        pack_options.bruteForce = brute_force != 0;
        pack_options.createImage = false;
        pack_options.rotateChartsToAxis = true;
        pack_options.rotateCharts = true;
        xatlas::Generate(handle->atlas, chart_options, pack_options);
    } catch (...) {
        // All output getters remain bounded and validation in the safe Rust wrapper reports failure.
    }
}

std::uint32_t metrocalk_xatlas_width(const MetrocalkXatlas *handle) noexcept {
    return handle != nullptr && handle->atlas != nullptr ? handle->atlas->width : 0;
}

std::uint32_t metrocalk_xatlas_height(const MetrocalkXatlas *handle) noexcept {
    return handle != nullptr && handle->atlas != nullptr ? handle->atlas->height : 0;
}

std::uint32_t metrocalk_xatlas_atlas_count(const MetrocalkXatlas *handle) noexcept {
    return handle != nullptr && handle->atlas != nullptr ? handle->atlas->atlasCount : 0;
}

std::uint32_t metrocalk_xatlas_chart_count(const MetrocalkXatlas *handle) noexcept {
    return handle != nullptr && handle->atlas != nullptr ? handle->atlas->chartCount : 0;
}

std::uint32_t metrocalk_xatlas_mesh_count(const MetrocalkXatlas *handle) noexcept {
    return handle != nullptr && handle->atlas != nullptr ? handle->atlas->meshCount : 0;
}

float metrocalk_xatlas_texels_per_unit(const MetrocalkXatlas *handle) noexcept {
    return handle != nullptr && handle->atlas != nullptr ? handle->atlas->texelsPerUnit : 0.0f;
}

float metrocalk_xatlas_utilization(const MetrocalkXatlas *handle, std::uint32_t atlas_index) noexcept {
    if (handle == nullptr || handle->atlas == nullptr || handle->atlas->utilization == nullptr ||
        atlas_index >= handle->atlas->atlasCount) {
        return -1.0f;
    }
    return handle->atlas->utilization[atlas_index];
}

std::uint32_t metrocalk_xatlas_mesh_vertex_count(
    const MetrocalkXatlas *handle,
    std::uint32_t mesh_index) noexcept {
    if (handle == nullptr || handle->atlas == nullptr || mesh_index >= handle->atlas->meshCount) {
        return 0;
    }
    return handle->atlas->meshes[mesh_index].vertexCount;
}

std::uint32_t metrocalk_xatlas_mesh_index_count(
    const MetrocalkXatlas *handle,
    std::uint32_t mesh_index) noexcept {
    if (handle == nullptr || handle->atlas == nullptr || mesh_index >= handle->atlas->meshCount) {
        return 0;
    }
    return handle->atlas->meshes[mesh_index].indexCount;
}

std::uint8_t metrocalk_xatlas_copy_mesh_vertices(
    const MetrocalkXatlas *handle,
    std::uint32_t mesh_index,
    MetrocalkXatlasVertex *output,
    std::uint32_t capacity) noexcept {
    if (handle == nullptr || handle->atlas == nullptr || mesh_index >= handle->atlas->meshCount ||
        output == nullptr) {
        return 0;
    }
    const xatlas::Mesh &mesh = handle->atlas->meshes[mesh_index];
    if (capacity != mesh.vertexCount || mesh.vertexArray == nullptr) {
        return 0;
    }
    for (std::uint32_t index = 0; index < mesh.vertexCount; ++index) {
        const xatlas::Vertex &source = mesh.vertexArray[index];
        output[index].atlas_index = source.atlasIndex;
        output[index].chart_index = source.chartIndex;
        output[index].uv[0] = source.uv[0];
        output[index].uv[1] = source.uv[1];
        output[index].xref = source.xref;
    }
    return 1;
}

std::uint8_t metrocalk_xatlas_copy_mesh_indices(
    const MetrocalkXatlas *handle,
    std::uint32_t mesh_index,
    std::uint32_t *output,
    std::uint32_t capacity) noexcept {
    if (handle == nullptr || handle->atlas == nullptr || mesh_index >= handle->atlas->meshCount ||
        output == nullptr) {
        return 0;
    }
    const xatlas::Mesh &mesh = handle->atlas->meshes[mesh_index];
    if (capacity != mesh.indexCount || mesh.indexArray == nullptr) {
        return 0;
    }
    for (std::uint32_t index = 0; index < mesh.indexCount; ++index) {
        output[index] = mesh.indexArray[index];
    }
    return 1;
}

} // extern "C"

