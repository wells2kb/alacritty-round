#if defined(GLES2_RENDERER)
attribute vec2 aPos;
attribute vec4 aColor;

#else
layout (location = 0) in vec3 aPos;
layout (location = 1) in vec4 aCornerFlags;
layout (location = 2) in vec4 aColor;

flat out vec4 color;
flat out vec2 rectPosition;
flat out float rectWidth;
flat out uint cornerFlags;
#endif

void main() {
    color = aColor;
    gl_Position = vec4(aPos.x, aPos.y, 0.0, 1.0);
    rectPosition = gl_Position.xy;
    rectWidth = aPos.z;
    cornerFlags = uint(aCornerFlags);
}
