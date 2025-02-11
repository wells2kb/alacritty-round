#if defined(GLES2_RENDERER)
attribute vec3 aPos;
attribute vec4 aCornerFlags;
attribute vec4 aColor;

varying mediump vec4 color;
#else
layout (location = 0) in vec3 aPos;
layout (location = 1) in vec4 aCornerFlags;
layout (location = 2) in vec4 aColor;

flat out vec4 color;
#endif

flat out vec2 rectPosition;
flat out float rectWidth;
flat out uint cornerFlags;

void main() {
    rectPosition = vec2(aPos.x, aPos.y);
    cornerFlags = uint(aCornerFlags);
    if (((cornerFlags & uint(3)) == uint(3)) && (gl_VertexID % 6 == 1 || gl_VertexID % 6 == 4)) {
        gl_Position = vec4(aPos.x + 0.01, aPos.y, 0.0, 1.0);
    }
    else if (((cornerFlags & uint(12)) == uint(12)) && (gl_VertexID % 6 == 2)) {
        gl_Position = vec4(aPos.x - 0.01, aPos.y, 0.0, 1.0);
    }
    else if (((cornerFlags & uint(48)) == uint(48)) && (gl_VertexID % 6 == 0 || gl_VertexID % 6 == 5)) {
        gl_Position = vec4(aPos.x - 0.01, aPos.y, 0.0, 1.0);
    }
    else if (((cornerFlags & uint(192)) == uint(192)) && (gl_VertexID % 6 == 3)) {
        gl_Position = vec4(aPos.x + 0.01, aPos.y, 0.0, 1.0);
    }
    else {
        gl_Position = vec4(aPos.x, aPos.y, 0.0, 1.0);
    }

    color = aColor;
    rectWidth = aPos.z;
}
