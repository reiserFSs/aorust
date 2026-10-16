#include <metal_stdlib>
using namespace metal;
// Local to this library: world/material shaders retain their existing native math.
#pragma METAL fp math_mode(safe)
#pragma METAL fp contract(off)

struct NativeVertex {
    float local0[3]; float weight;
    float local1[3]; uint bone0;
    float normal[3]; uint bone1;
};
struct Bone { float4 rows[3]; };
inline float rotate_row(float4 row, thread const float* v) {
    float x = row.x * v[0];
    float y = row.y * v[1];
    float xy = x + y;
    float z = row.z * v[2];
    return xy + z;
}
kernel void skin(device const NativeVertex* native [[buffer(0)]],
                 device const Bone* bones [[buffer(1)]],
                 device float* vertices [[buffer(2)]],
                 constant uint4& vertex_count [[buffer(3)]],
                 uint i [[thread_position_in_grid]]) {
    if (i >= vertex_count.x) return;
    NativeVertex v = native[i];
    Bone b0 = bones[v.bone0];
    float p[3];
    float n[3];
    for (uint j = 0; j < 3; ++j) {
        float p0 = rotate_row(b0.rows[j], v.local0) + b0.rows[j].w;
        p[j] = p0;
        if (!(v.weight >= 0.999f)) {
            float4 row = bones[v.bone1].rows[j];
            float p1 = rotate_row(row, v.local1) + row.w;
            float a = v.weight * p0;
            float b = (1.0f - v.weight) * p1;
            p[j] = a + b;
        }
        n[j] = rotate_row(b0.rows[j], v.normal);
    }
    float xx = n[0] * n[0];
    float yy = n[1] * n[1];
    float xy = xx + yy;
    float zz = n[2] * n[2];
    float length = precise::sqrt(xy + zz);
    if (length > 1e-12f) {
        n[0] = precise::divide(n[0], length);
        n[1] = precise::divide(n[1], length);
        n[2] = precise::divide(n[2], length);
    } else { n[0] = 0.0f; n[1] = 1.0f; n[2] = 0.0f; }
    vertices[i * 12] = p[0];
    vertices[i * 12 + 1] = p[1];
    vertices[i * 12 + 2] = -p[2];
    vertices[i * 12 + 3] = n[0];
    vertices[i * 12 + 4] = n[1];
    vertices[i * 12 + 5] = -n[2];
}
