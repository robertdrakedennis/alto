// The real cache BatchedSprite GLSL consumes quads exported by Java.
#define TEXT_PIXELS_HELPERS_ONLY
#include "text-pixels.cpp"
int main(int argc,char**argv){try{
    std::string dir=argv[1];CGLPixelFormatAttribute attrs[]={kCGLPFAAccelerated,kCGLPFAAllowOfflineRenderers,(CGLPixelFormatAttribute)0};CGLPixelFormatObj format;CGLContextObj ctx;GLint n;
    if(CGLChoosePixelFormat(attrs,&format,&n)!=kCGLNoError||CGLCreateContext(format,nullptr,&ctx)!=kCGLNoError)throw std::runtime_error("CGL context");CGLDestroyPixelFormat(format);CGLSetCurrentContext(ctx);
    GLuint program=glCreateProgram();glAttachShader(program,shader(GL_VERTEX_SHADER,"#version 120\n"+read(dir+"/vertex.glsl")));glAttachShader(program,shader(GL_FRAGMENT_SHADER,"#version 120\n"+read(dir+"/fragment.glsl")));glLinkProgram(program);GLint linked;glGetProgramiv(program,GL_LINK_STATUS,&linked);if(!linked)throw std::runtime_error("sprite link");glUseProgram(program);glUniform1i(glGetUniformLocation(program,"SpriteSampler"),0);
    std::ifstream input(dir+"/input.bin",std::ios::binary),quads(dir+"/java-quads.bin",std::ios::binary);std::ofstream out(dir+"/reference.rgba",std::ios::binary);
    quads.ignore(65536*8);unsigned count=u32(input),rendered=0;
    for(unsigned c=0;c<count;c++){
        bool gpu=u32(input);int w=u32(input),h=u32(input);input.ignore(16);unsigned sprites=u32(input);std::vector<GLuint> textures(sprites);GLuint fb=0,target=0;
        if(gpu){glGenFramebuffersEXT(1,&fb);glBindFramebufferEXT(GL_FRAMEBUFFER_EXT,fb);glGenTextures(1,&target);glBindTexture(GL_TEXTURE_2D,target);glTexImage2D(GL_TEXTURE_2D,0,GL_RGBA8,w,h,0,GL_RGBA,GL_UNSIGNED_BYTE,nullptr);glFramebufferTexture2DEXT(GL_FRAMEBUFFER_EXT,GL_COLOR_ATTACHMENT0_EXT,GL_TEXTURE_2D,target,0);if(glCheckFramebufferStatusEXT(GL_FRAMEBUFFER_EXT)!=GL_FRAMEBUFFER_COMPLETE_EXT)throw std::runtime_error("FBO");glGenTextures(sprites,textures.data());}
        for(unsigned i=0;i<sprites;i++){
            int tw=u32(input),th=u32(input);input.ignore(16);std::vector<unsigned char> tex(tw*th*4);
            for(int k=0;k<tw*th;k++){unsigned p=u32(input);tex[k*4]=p>>16;tex[k*4+1]=p>>8;tex[k*4+2]=p;tex[k*4+3]=p>>24;}
            if(gpu){glBindTexture(GL_TEXTURE_2D,textures[i]);glTexImage2D(GL_TEXTURE_2D,0,GL_RGBA8,tw,th,0,GL_RGBA,GL_UNSIGNED_BYTE,tex.data());glTexParameteri(GL_TEXTURE_2D,GL_TEXTURE_MIN_FILTER,GL_LINEAR);glTexParameteri(GL_TEXTURE_2D,GL_TEXTURE_MAG_FILTER,GL_LINEAR);glTexParameteri(GL_TEXTURE_2D,GL_TEXTURE_WRAP_S,GL_REPEAT);glTexParameteri(GL_TEXTURE_2D,GL_TEXTURE_WRAP_T,GL_REPEAT);}
        }
        if(gpu){glViewport(0,0,w,h);glDisable(GL_SCISSOR_TEST);glDisable(GL_DEPTH_TEST);glDisable(GL_CULL_FACE);glDisable(GL_DITHER);glEnable(GL_ALPHA_TEST);glAlphaFunc(GL_GREATER,0);glEnable(GL_BLEND);glBlendFuncSeparate(GL_SRC_ALPHA,GL_ONE_MINUS_SRC_ALPHA,GL_ZERO,GL_ZERO);glClearColor(0.1,0.2,0.3,0.4);glClear(GL_COLOR_BUFFER_BIT);}
        unsigned ops=u32(input);for(unsigned i=0;i<ops;i++){
            input.ignore(18*4);int ok=u32(quads);quads.ignore(16);unsigned num=u32(quads);if(gpu&&!ok)throw std::runtime_error("GPU frame has failure");
            for(unsigned q=0;q<num;q++){
                unsigned tex=u32(quads);int clip[4];for(int&v:clip)v=u32(quads);float v[4][4];for(auto&r:v)for(float&f:r)f=f32(quads);unsigned colour=u32(quads);
                if(!gpu||clip[0]>=clip[2]||clip[1]>=clip[3])continue;
                glBindTexture(GL_TEXTURE_2D,textures.at(tex));glEnable(GL_SCISSOR_TEST);glScissor(clip[0],h-clip[3],clip[2]-clip[0],clip[3]-clip[1]);
                glColor4ub(colour>>16,colour>>8,colour,colour>>24);glBegin(GL_TRIANGLES);for(int index:{0,2,1,2,3,1}){glMultiTexCoord2f(GL_TEXTURE0,v[index][2],v[index][3]);glVertex3f(v[index][0],v[index][1],0);}glEnd();
            }
        }
        if(gpu){std::vector<unsigned char> pixels(w*h*4);glReadPixels(0,0,w,h,GL_RGBA,GL_UNSIGNED_BYTE,pixels.data());if(glGetError())throw std::runtime_error("GL sprite pixels");for(int y=h-1;y>=0;y--)out.write((char*)pixels.data()+y*w*4,w*4);glDeleteTextures(1,&target);glDeleteTextures(sprites,textures.data());glDeleteFramebuffersEXT(1,&fb);rendered++;}
    }
    if(input.peek()!=EOF||quads.peek()!=EOF)throw std::runtime_error("unread sprite data");
    std::cerr<<"GL sprite frames: "<<rendered<<" on "<<glGetString(GL_RENDERER)<<"\n";CGLSetCurrentContext(nullptr);CGLDestroyContext(ctx);return 0;
}catch(std::exception&e){std::cerr<<e.what()<<"\n";return 1;}}
