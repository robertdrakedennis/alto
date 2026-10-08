// Runs the unmodified cache GLSL in a macOS compatibility OpenGL context.
// CGL only replaces Java's window; shader source and GLX draw-state semantics
// are the reference. Output rows are flipped to WebGPU readback order.
#include <OpenGL/OpenGL.h>
#include <OpenGL/gl.h>
#include <OpenGL/glext.h>
#include <fstream>
#include <sstream>
#include <vector>
#include <string>
#include <iostream>
#include <stdexcept>

std::string read(const std::string& p) {std::ifstream f(p,std::ios::binary);if(!f)throw std::runtime_error(p);return std::string(std::istreambuf_iterator<char>(f),{});}
GLuint shader(GLenum kind,std::string text) {
    GLuint s=glCreateShader(kind);const char* p=text.c_str();glShaderSource(s,1,&p,nullptr);glCompileShader(s);GLint ok=0;glGetShaderiv(s,GL_COMPILE_STATUS,&ok);
    if(!ok){char log[8192];glGetShaderInfoLog(s,sizeof(log),nullptr,log);throw std::runtime_error(log);}return s;
}
void u3(GLuint p,const char* name,float x,float y,float z){glUniform3f(glGetUniformLocation(p,name),x,y,z);}
void u4(GLuint p,const char* name,float x,float y,float z,float w){glUniform4f(glGetUniformLocation(p,name),x,y,z,w);}
int main(int argc,char** argv) {try {
    if(argc!=3)throw std::runtime_error("material-pixels <shader-directory> <output-file>");
    CGLPixelFormatAttribute attr[]={kCGLPFAAccelerated,kCGLPFAAllowOfflineRenderers,(CGLPixelFormatAttribute)0};
    CGLPixelFormatObj format;GLint n;CGLContextObj ctx;
    if(CGLChoosePixelFormat(attr,&format,&n)!=kCGLNoError||CGLCreateContext(format,nullptr,&ctx)!=kCGLNoError)throw std::runtime_error("CGL context failed");
    CGLDestroyPixelFormat(format);CGLSetCurrentContext(ctx);
    std::cerr<<"[material-pixels] "<<glGetString(GL_RENDERER)<<"\n";
    GLuint framebuffer,target,depth;glGenFramebuffersEXT(1,&framebuffer);glBindFramebufferEXT(GL_FRAMEBUFFER_EXT,framebuffer);
    glGenTextures(1,&target);glBindTexture(GL_TEXTURE_2D,target);glTexImage2D(GL_TEXTURE_2D,0,GL_RGBA8,32,32,0,GL_RGBA,GL_UNSIGNED_BYTE,nullptr);
    glFramebufferTexture2DEXT(GL_FRAMEBUFFER_EXT,GL_COLOR_ATTACHMENT0_EXT,GL_TEXTURE_2D,target,0);
    glGenRenderbuffersEXT(1,&depth);glBindRenderbufferEXT(GL_RENDERBUFFER_EXT,depth);glRenderbufferStorageEXT(GL_RENDERBUFFER_EXT,GL_DEPTH_COMPONENT24,32,32);
    glFramebufferRenderbufferEXT(GL_FRAMEBUFFER_EXT,GL_DEPTH_ATTACHMENT_EXT,GL_RENDERBUFFER_EXT,depth);
    if(glCheckFramebufferStatusEXT(GL_FRAMEBUFFER_EXT)!=GL_FRAMEBUFFER_COMPLETE_EXT)throw std::runtime_error("incomplete FBO");
    GLuint diffuse,cube,noise;glGenTextures(1,&diffuse);glGenTextures(1,&cube);glGenTextures(1,&noise);
    glActiveTexture(GL_TEXTURE1);glBindTexture(GL_TEXTURE_CUBE_MAP,cube);
    for(int face=0;face<6;face++) {
        unsigned char pixels[64];int at=0;
        for(int y=0;y<4;y++)for(int x=0;x<4;x++) {pixels[at++]=40+45*x+10*face;pixels[at++]=30+50*y+5*face;pixels[at++]=20+35*face;pixels[at++]=255;}
        glTexImage2D(GL_TEXTURE_CUBE_MAP_POSITIVE_X+face,0,GL_RGBA8,4,4,0,GL_RGBA,GL_UNSIGNED_BYTE,pixels);
    }
    glTexParameteri(GL_TEXTURE_CUBE_MAP,GL_TEXTURE_MIN_FILTER,GL_LINEAR);glTexParameteri(GL_TEXTURE_CUBE_MAP,GL_TEXTURE_MAG_FILTER,GL_LINEAR);
    // GlxCubeTexture/GlxBaseTexture leave cube wrapping at its GL defaults.
    GLint wrap=0;glGetTexParameteriv(GL_TEXTURE_CUBE_MAP,GL_TEXTURE_WRAP_S,&wrap);std::cerr<<"[material-pixels] cube wrap "<<wrap<<"\n";
    const std::string volume=read("tools/client910/assets/water-billow.bin");glActiveTexture(GL_TEXTURE0);glBindTexture(GL_TEXTURE_3D,noise);
    glTexImage3D(GL_TEXTURE_3D,0,GL_LUMINANCE8_ALPHA8,128,128,16,0,GL_LUMINANCE_ALPHA,GL_UNSIGNED_BYTE,volume.data());
    glGenerateMipmapEXT(GL_TEXTURE_3D);glTexParameteri(GL_TEXTURE_3D,GL_TEXTURE_MIN_FILTER,GL_LINEAR_MIPMAP_LINEAR);glTexParameteri(GL_TEXTURE_3D,GL_TEXTURE_MAG_FILTER,GL_LINEAR);
    std::ofstream output(argv[2],std::ios::binary);std::string dir=argv[1];
    // mode, exponent, diffuse alpha, alpha reference, point-light count, reverse winding.
    const int cases[][6]={{0,0,255,0,0,0},{0,0,0,0,0,0},{0,0,127,127,0,0},{0,0,128,127,0,0},{0,0,127,128,0,0},{0,0,255,0,0,1},
        {1,32,128,0,0,0},{1,4,128,0,0,0},{1,1,128,0,0,0},{1,0,128,0,0,0},{2,0,128,0,0,0},{3,0,128,0,0,0},{6,0,0,0,0,0},{0,0,255,0,1,0},{1,4,128,0,1,0},{5,0,255,0,0,0},{0,0,255,0,4,0},{2,0,128,0,0,0},{2,0,128,0,0,0},{2,0,128,0,0,0},{2,0,128,0,0,0},{2,0,128,0,0,0},{2,0,128,0,0,0},{2,0,128,0,0,0},{2,0,128,0,0,0},{2,0,128,0,0,0},{2,0,128,0,0,0},{2,0,128,0,0,0},{5,0,255,0,0,0},{5,0,255,0,0,0}};
    int caseIndex=-1;
    for(const auto& c:cases) {
        caseIndex++;
        int mode=c[0];bool water=mode==5;std::string defines="#version 120\n#define ShaderMode "+std::to_string(mode==6?3:mode)+"\n#define IgnoreAlpha "+(mode==6?"true":"false")+"\n#define PointLightCount "+std::to_string(c[4])+"\n";
        GLuint p=glCreateProgram();glAttachShader(p,shader(GL_VERTEX_SHADER,defines+read(dir+(water?"/27.bin":"/12.bin"))));glAttachShader(p,shader(GL_FRAGMENT_SHADER,defines+read(dir+(water?"/17.bin":"/23.bin"))));glLinkProgram(p);
        GLint ok;glGetProgramiv(p,GL_LINK_STATUS,&ok);if(!ok){char log[8192];glGetProgramInfoLog(p,sizeof(log),nullptr,log);throw std::runtime_error(log);}glUseProgram(p);
        const float wvp[]={1,0,0,0,0,1,0,0,0,0,2,0,0,0,-1,1};glUniform4fv(glGetUniformLocation(p,"WVPMatrix"),4,wvp);
        float world[]={1,0,0,0,0,1,0,0,0,0,1,0,0,0,0,1};
        const float rotations[][16]={{0,0,-1,0,0,1,0,0,1,0,0,0,0,0,0,1},{1,0,0,0,0,0,1,0,0,-1,0,0,0,0,0,1},{0,1,0,0,-1,0,0,0,0,0,1,0,0,0,0,1},{0,0,1,0,1,0,0,0,0,1,0,0,0,0,0,1}};
        if(caseIndex>=24){for(int k=0;k<16;k++)world[k]=rotations[(caseIndex-24)%4][k];if(water){world[12]=0.125f;world[13]=0.5f;world[14]=-0.25f;}}
        if(water)glUniform4fv(glGetUniformLocation(p,"WorldMatrix"),4,world);else {float rot[]={world[0],world[1],world[2],world[4],world[5],world[6],world[8],world[9],world[10]};glUniform3fv(glGetUniformLocation(p,"WorldMatrix"),3,rot);}
        const float uv[]={1,0,0,1,0,0,0,0};glUniform2fv(glGetUniformLocation(p,"TexCoordMatrix"),4,uv);
        // Avoid exact major-axis ties: the two rasterizers interpolate those
        // boundary values differently. Both sides of each border remain covered.
        const float eyes[][3]={{-8,0,0.25f},{8,0,0.25f},{0,8,0.25f},{0,-8,0.25f},{0,0,8},{2.1f,2.2f,0.25f},{0.1f,2.1f,2.1f}};
        if(caseIndex>=17)u3(p,"EyePos",eyes[(caseIndex-17)%7][0],eyes[(caseIndex-17)%7][1],eyes[(caseIndex-17)%7][2]);else u3(p,"EyePos",0,0,-2);u3(p,"SunDir",0,0,-1);u3(p,"SunColour",0.3f,0.4f,0.5f);u3(p,"AntiSunColour",0,0,0);u3(p,"AmbientColour",0.2f,0.2f,0.2f);
        u4(p,"SpecularExponent",c[1],0,0,0);u4(p,"HeightFogPlane",0,0,0,0);u4(p,"DistanceFogPlane",0,0,0,0.25f);
        u3(p,"HeightFogColour",0,0,0);u3(p,"DistanceFogColour",0.1f,0.2f,0.3f);
        const float lightPos[]={0,0,-2,0.25f,0,0,-2,0.25f,0,0,-2,0.25f,0,0,-2,0.25f};
        const float lightRGB[]={0.1f,0.2f,0.3f,1,0.1f,0.2f,0.3f,1,0.1f,0.2f,0.3f,1,0.1f,0.2f,0.3f,1};
        if(c[4]>0) {glUniform4fv(glGetUniformLocation(p,"PointLightsPosAndRadiusInv"),c[4],lightPos);glUniform4fv(glGetUniformLocation(p,"PointLightsDiffuseColour"),c[4],lightRGB);}
        u4(p,"UGenerationPlane",1,0,0,0);u4(p,"VGenerationPlane",0,1,0,0);u4(p,"Time",0.25f,0,0,0);
        glUniform1i(glGetUniformLocation(p,"DiffuseSampler"),0);glUniform1i(glGetUniformLocation(p,"EnvironmentSampler"),1);glUniform1i(glGetUniformLocation(p,"billowSampler3D"),0);
        glActiveTexture(GL_TEXTURE0);glBindTexture(GL_TEXTURE_2D,diffuse);const unsigned char rgba[]={160,120,80,(unsigned char)c[2]};glTexImage2D(GL_TEXTURE_2D,0,GL_RGBA8,1,1,0,GL_RGBA,GL_UNSIGNED_BYTE,rgba);glTexParameteri(GL_TEXTURE_2D,GL_TEXTURE_MIN_FILTER,GL_NEAREST);glTexParameteri(GL_TEXTURE_2D,GL_TEXTURE_MAG_FILTER,GL_NEAREST);
        glViewport(0,0,32,32);glDisable(GL_DITHER);glDepthMask(GL_TRUE);glClearColor(0.1f,0.2f,0.3f,0.4f);glClear(GL_COLOR_BUFFER_BIT|GL_DEPTH_BUFFER_BIT);
        glEnable(GL_DEPTH_TEST);glDepthFunc(GL_LEQUAL);glEnable(GL_ALPHA_TEST);glAlphaFunc(GL_GREATER,c[3]/255.f);
        if(c[3])glDisable(GL_BLEND);else {glEnable(GL_BLEND);glBlendFuncSeparate(GL_SRC_ALPHA,GL_ONE_MINUS_SRC_ALPHA,GL_ZERO,GL_ZERO);}
        glEnable(GL_CULL_FACE);glCullFace(GL_BACK);glFrontFace(GL_CCW);
        glColor4ub(200,180,160,255);glNormal3f(0,0,-1);glMultiTexCoord2f(GL_TEXTURE0,0.5f,0.5f);
        glBegin(GL_TRIANGLES);glVertex3f(-1,-1,0.25f);if(c[5]) {glVertex3f(-1,3,0.25f);glVertex3f(3,-1,0.25f);}else {glVertex3f(3,-1,0.25f);glVertex3f(-1,3,0.25f);}glEnd();
        std::vector<unsigned char> pixels(32*32*4);glReadPixels(0,0,32,32,GL_RGBA,GL_UNSIGNED_BYTE,pixels.data());
        GLenum err=glGetError();if(err)throw std::runtime_error("OpenGL error "+std::to_string(err));
        for(int y=31;y>=0;y--)output.write((char*)pixels.data()+y*128,128);
        glDeleteProgram(p);
    }
    std::cerr<<"[material-pixels] 30 cases written\n";CGLSetCurrentContext(nullptr);CGLDestroyContext(ctx);return 0;
} catch(const std::exception& e){std::cerr<<e.what()<<"\n";return 1;}}
