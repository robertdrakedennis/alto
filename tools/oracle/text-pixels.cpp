#include <OpenGL/OpenGL.h>
#include <OpenGL/gl.h>
#include <OpenGL/glext.h>
#include <fstream>
#include <vector>
#include <string>
#include <iostream>
#include <stdexcept>
#include <cstring>
unsigned u32(std::istream& in){unsigned char b[4];in.read((char*)b,4);if(!in)throw std::runtime_error("input EOF");return unsigned(b[0])<<24|unsigned(b[1])<<16|unsigned(b[2])<<8|b[3];}
float f32(std::istream& in){unsigned u=u32(in);float f;std::memcpy(&f,&u,4);return f;}
std::string read(std::string p){std::ifstream in(p);if(!in)throw std::runtime_error(p);return {std::istreambuf_iterator<char>(in),{}};}
GLuint shader(GLenum kind,std::string src){GLuint s=glCreateShader(kind);const char*p=src.c_str();glShaderSource(s,1,&p,nullptr);glCompileShader(s);GLint ok;glGetShaderiv(s,GL_COMPILE_STATUS,&ok);if(!ok){char log[8192];glGetShaderInfoLog(s,sizeof(log),nullptr,log);throw std::runtime_error(log);}return s;}
#ifndef TEXT_PIXELS_HELPERS_ONLY
int main(int argc,char**argv){try{
    std::string dir=argv[1];CGLPixelFormatAttribute attrs[]={kCGLPFAAccelerated,kCGLPFAAllowOfflineRenderers,(CGLPixelFormatAttribute)0};CGLPixelFormatObj format;CGLContextObj ctx;GLint n;
    if(CGLChoosePixelFormat(attrs,&format,&n)!=kCGLNoError||CGLCreateContext(format,nullptr,&ctx)!=kCGLNoError)throw std::runtime_error("CGL context");CGLDestroyPixelFormat(format);CGLSetCurrentContext(ctx);
    GLuint program=glCreateProgram();glAttachShader(program,shader(GL_VERTEX_SHADER,"#version 120\n"+read(dir+"/vertex.glsl")));glAttachShader(program,shader(GL_FRAGMENT_SHADER,"#version 120\n"+read(dir+"/fragment.glsl")));glLinkProgram(program);GLint linked;glGetProgramiv(program,GL_LINK_STATUS,&linked);if(!linked)throw std::runtime_error("text link");glUseProgram(program);glUniform1i(glGetUniformLocation(program,"SpriteSampler"),0);
    std::ifstream input(dir+"/input.bin",std::ios::binary),quads(dir+"/java-quads.bin",std::ios::binary);std::ofstream out(dir+"/reference.rgba",std::ios::binary);
    unsigned count=u32(input);for(unsigned c=0;c<count;c++){
        int w=u32(input),h=u32(input);for(int k=0;k<4;k++)u32(input);int tw=u32(input),th=u32(input),scale=u32(input);std::vector<unsigned char> tex(tw*th*4);
        for(int k=0;k<tw*th;k++){unsigned p=u32(input);tex[k*4]=p>>16;tex[k*4+1]=p>>8;tex[k*4+2]=p;tex[k*4+3]=p>>24;}
        GLuint fb,target,atlas;glGenFramebuffersEXT(1,&fb);glBindFramebufferEXT(GL_FRAMEBUFFER_EXT,fb);glGenTextures(1,&target);glBindTexture(GL_TEXTURE_2D,target);glTexImage2D(GL_TEXTURE_2D,0,GL_RGBA8,w,h,0,GL_RGBA,GL_UNSIGNED_BYTE,nullptr);glFramebufferTexture2DEXT(GL_FRAMEBUFFER_EXT,GL_COLOR_ATTACHMENT0_EXT,GL_TEXTURE_2D,target,0);if(glCheckFramebufferStatusEXT(GL_FRAMEBUFFER_EXT)!=GL_FRAMEBUFFER_COMPLETE_EXT)throw std::runtime_error("FBO");
        glGenTextures(1,&atlas);glBindTexture(GL_TEXTURE_2D,atlas);glTexImage2D(GL_TEXTURE_2D,0,GL_RGBA8,tw,th,0,GL_RGBA,GL_UNSIGNED_BYTE,tex.data());glTexParameteri(GL_TEXTURE_2D,GL_TEXTURE_MIN_FILTER,scale==1?GL_NEAREST:GL_LINEAR);glTexParameteri(GL_TEXTURE_2D,GL_TEXTURE_MAG_FILTER,scale==1?GL_NEAREST:GL_LINEAR);
        glViewport(0,0,w,h);glDisable(GL_DEPTH_TEST);glDisable(GL_CULL_FACE);glDisable(GL_DITHER);glEnable(GL_ALPHA_TEST);glAlphaFunc(GL_GREATER,0);glEnable(GL_BLEND);glBlendFuncSeparate(GL_SRC_ALPHA,GL_ONE_MINUS_SRC_ALPHA,GL_ZERO,GL_ZERO);glClearColor(0.1,0.2,0.3,0.4);glClear(GL_COLOR_BUFFER_BIT);
        unsigned num=u32(input);for(unsigned i=0;i<num;i++){input.ignore(36);unsigned present=u32(quads);if(!present)continue;float v[4][4];for(auto&r:v)for(float&f:r)f=f32(quads);unsigned colour=u32(quads);glColor4ub(colour>>16,colour>>8,colour,colour>>24);glBegin(GL_TRIANGLES);for(int index:{0,2,1,2,3,1}){glMultiTexCoord2f(GL_TEXTURE0,v[index][2],v[index][3]);glVertex3f(v[index][0],v[index][1],0);}glEnd();}
        std::vector<unsigned char> pixels(w*h*4);glReadPixels(0,0,w,h,GL_RGBA,GL_UNSIGNED_BYTE,pixels.data());if(glGetError())throw std::runtime_error("GL text pixels");for(int y=h-1;y>=0;y--)out.write((char*)pixels.data()+y*w*4,w*4);glDeleteTextures(1,&target);glDeleteTextures(1,&atlas);glDeleteFramebuffersEXT(1,&fb);
    }std::cerr<<"GL text frames: "<<count<<" on "<<glGetString(GL_RENDERER)<<"\n";CGLSetCurrentContext(nullptr);CGLDestroyContext(ctx);return 0;
}catch(std::exception&e){std::cerr<<e.what()<<"\n";return 1;}}

#endif
