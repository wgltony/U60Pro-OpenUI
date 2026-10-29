/* 320x480 local touchscreen keyboard. Every visible key is an HTML hit action.
 * Narrow character keys are 30x44; editing/action controls are >=44 pixels high. */
static void relay_keybox(int x,int y,int w,const char *id,const char *label) {
  add("<a id=\"%s\" href=\"act:%s\" class=\"box btn\" style=\"left:%dpx;top:%dpx;width:%dpx;height:44px;line-height:42px\">",id,id,x,y,w);
  shortened(label,w-4,16);add("</a>");
}
static void render_relay_keyboard(void) {
  header(relay_edit.key==RK_SSID?L("SSID (ASCII)","SSID (ASCII)"):L("Wi-Fi password","Wi-Fi 密码"));
  /* The HTML scaffold contains no input, even when Show is selected. Input and
   * count are painted into this reserved 296x64 rectangle without replacing g_doc. */
  relay_keybox(8,136,94,"rclear",L("Clear","清空"));
  relay_keybox(110,136,94,"rshow",relay_edit.key==RK_PASSWORD?(relay_edit.show?L("Hide","隐藏"):L("Show","显示")):L("ASCII","ASCII"));
  relay_keybox(212,136,100,"rbs",L("⌫ Delete","⌫ 退格"));
  for(int row=0;row<3;row++) {
    const char *keys=relay_key_row(row);int len=(int)strlen(keys),start=(320-len*31)/2;
    for(int i=0;i<len;i++) {
      char id[8],label[2]={keys[i],0};snprintf(id,sizeof(id),"rk%02x",(unsigned char)keys[i]);
      relay_keybox(start+i*31,190+row*48,30,id,label);
    }
  }
  relay_keybox(8,336,70,"rshift",L("Aa","大小写"));
  relay_keybox(86,336,70,"rsymbol",relay_edit.keymode==2?"#+=":"123!?");
  relay_keybox(164,336,70,"rletters","ABC");
  relay_keybox(242,336,70,"rspace",L("Space","空格"));
  linkbox(8,396,148,52,"rkcancel",L("Cancel","取消"),1,PLAIN);
  linkbox(164,396,148,52,"rkdone",L("Done","完成"),1,PRIMARY);
}
/* Cache only public, fixed-font ASCII metrics once; no per-key FreeType measuring
 * and no credential-derived cache. Bounds include the native bold pass's +1 pixel. */
static struct {int advance,left,right;} relay_keyboard_glyph[95];
static int relay_keyboard_glyph_ready;
static void relay_keyboard_glyph_init(void) {
  if(relay_keyboard_glyph_ready)return;
  for(int c=32;c<=126;c++) {
    char glyph[2]={(char)c,0};int x0,y0,x1,y1;
    relay_keyboard_glyph[c-32].advance=html_view_text_width_px(glyph,16);
    html_view_text_bounds_px(glyph,16,&x0,&y0,&x1,&y1);
    relay_keyboard_glyph[c-32].left=x0;relay_keyboard_glyph[c-32].right=x1+1;
  }
  relay_keyboard_glyph_ready=1;
}
static int relay_keyboard_text_extent(const char *value,int *left) {
  int pen=0,min=0,max=0;relay_keyboard_glyph_init();
  for(const unsigned char *s=(const unsigned char *)value;*s;s++) {
    if(*s<32||*s>126)continue;
    int i=*s-32,a=pen+relay_keyboard_glyph[i].left,b=pen+relay_keyboard_glyph[i].right;
    if(a<min)min=a;
    if(b>max)max=b;
    pen+=relay_keyboard_glyph[i].advance;if(pen>max)max=pen;
  }
  if(left)*left=min;
  return max-min;
}
/* Bounded ASCII display tail, fitted to the reserved rectangle; caller wipes scratch. */
static void relay_keyboard_display(char value[35],char count[64]) {
  const char *s=relay_input();size_t n=strlen(s),visible=n>34?34:n;
  if(relay_edit.key==RK_PASSWORD&&!relay_edit.show)memset(value,'*',visible);
  else memcpy(value,s+n-visible,visible);
  value[visible]=0;
  while(*value&&relay_keyboard_text_extent(value,NULL)>296)memmove(value,value+1,strlen(value));
  snprintf(count,64,L("%zu / %zu bytes%s","%zu / %zu 字节%s"),n,relay_input_limit(),relay_edit.overflow?L(" · FULL"," · 已满"):"");
}
static void relay_keyboard_paint(uint16_t *target) {
  char value[35]={0},count[64]={0};relay_keyboard_display(value,count);
  html_view_target_begin(target,0);
  /* Clear the whole old value/count, including a shown password's abandoned tail. */
  html_view_fill_rect(12,72,296,64,11,13,17,255);
  int left=0;relay_keyboard_text_extent(value,&left);
  html_view_draw_text_px(12-left,72,value,16,1,232,234,238,255);
  if(relay_edit.overflow)html_view_draw_text_px(12,106,count,16,0,240,106,106,255);
  else html_view_draw_text_px(12,106,count,16,0,154,161,172,255);
  html_view_target_end();
  relay_wipe(value,sizeof(value));relay_wipe(count,sizeof(count));
}
