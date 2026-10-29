/* SCREEN_PREVIEW only: synthetic credentials and mock IPC, never live Wi-Fi. */
static void relay_press(struct drm_buf *b,const char *id) {
  render(b);char selector[48];snprintf(selector,sizeof(selector),"#%s",id);
  int x,y,w,h;assert(html_view_rect(selector,&x,&y,&w,&h));
  assert(w>=30&&h>=44&&x>=0&&y>=0&&x+w<=W&&y+h<=H);
  tap(x+w/2,y+h/2);
}
static void relay_fixture(char *out,size_t cap,unsigned long long rev,unsigned long long gen) {
  unsigned long long now=(unsigned long long)time(NULL);
  size_t n=(size_t)snprintf(out,cap,"S 7 1 0 0 0 0 0 0 1 %llu\nR\t%llu\t1\tn\t-\tSynthetic\t-50\t5\t192.0.2.20\t0\t0\t0\t8\nL\t%llu\t%llu\tready\t%llu\t8\t6\n",now,rev,rev,gen,now);
  for(int i=0;i<8;i++)n+=(size_t)snprintf(out+n,cap-n,"l\t%d\t%d\twpa2\t0\t1\t4e6574%02x\n",i+1,i==0,'0'+i);
  for(int i=0;i<6;i++)n+=(size_t)snprintf(out+n,cap-n,"w\t%d\t%s\t-55\t2412\t5363616e%02x\n",i,i==5?"unsupported":"wpa2",'0'+i);
}
static void relay_type(struct drm_buf *b,const char *s) {
  for(;*s;s++) {
    int found=0;
    for(int mode=0;mode<4&&!found;mode++) {
      relay_edit.keymode=mode;
      for(int row=0;row<3&&!found;row++)if(strchr(relay_key_row(row),*s)) {
        char id[8];snprintf(id,sizeof(id),"rk%02x",(unsigned char)*s);relay_press(b,id);found=1;
      }
    }
    if(*s==' '){relay_press(b,"rspace");found=1;}assert(found);
  }
}
/* Compare the optimized logical framebuffer with a clean forced scaffold + paint.
 * Only synthetic fixture input is ever used; neither bytes nor timing log secrets. */
static void relay_render_reference(struct drm_buf *b,uint16_t *reference) {
  memcpy(reference,pixels,W*H*2);
  unsigned long long full=relay_keyboard_scaffold_draws,dynamic=relay_keyboard_dynamic_draws;
  int keyboard=relay_keyboard_active();assert(render_changed(b,1));
  assert(relay_keyboard_scaffold_draws==full+(keyboard?1:0)&&relay_keyboard_dynamic_draws==dynamic);
  assert(!memcmp(reference,pixels,W*H*2));
  for(int y=0;y<H;y++)assert(!memcmp((uint8_t*)b->map+y*b->pitch,pixels+y*W,W*2));
  assert(!render_changed(b,0));
}
/* expected: 1 keyboard scaffold, 0 cached text only, -1 leave keyboard. */
static void relay_cached_press(struct drm_buf *b,const char *id,int expected,uint16_t *reference) {
  assert(!render_changed(b,0));
  char selector[48];snprintf(selector,sizeof(selector),"#%s",id);
  int x,y,w,h;assert(html_view_rect(selector,&x,&y,&w,&h));
  assert(w>=30&&h>=44&&x>=0&&y>=0&&x+w<=W&&y+h<=H);
  unsigned long long full=relay_keyboard_scaffold_draws,dynamic=relay_keyboard_dynamic_draws;
  tap(x+w/2,y+h/2); /* Real existing HTML hit document, never coordinate dispatch. */
  assert(render_changed(b,0));
  assert(relay_keyboard_scaffold_draws==full+(expected==1));
  assert(relay_keyboard_dynamic_draws==dynamic+(expected==0));
  assert(!pending&&!test_log[0]&&!test_request[0]);
  relay_render_reference(b,reference);
}
static void relay_cached_render_tests(struct drm_buf *b) {
  uint16_t *reference=malloc(W*H*2);assert(reference);int zh=settings_zh;
  char fixture[4096];relay_fixture(fixture,sizeof(fixture),21,8);assert(parse_snapshot(fixture,&data));
  reset_job();page=UPLINK;dialog=0;test_log[0]=test_request[0]=0;
  for(int lang=0;lang<2;lang++) {
    /* Prior language finishes on MENU after the power/page-exit regression. */
    reset_job();page=UPLINK;dialog=0;test_log[0]=test_request[0]=0;
    settings_zh=lang;relay_begin();relay_edit.security=1;relay_view=RV_EDIT;relay_key_open(RK_PASSWORD);
    unsigned long long full=relay_keyboard_scaffold_draws;
    assert(render_changed(b,0)&&relay_keyboard_scaffold_draws==full+1);relay_render_reference(b,reference);
    /* Eight consecutive edits without any intervening full render: same g_doc. */
    int x,y,w,h;assert(html_view_rect("#rk61",&x,&y,&w,&h));
    unsigned long long dynamic=relay_keyboard_dynamic_draws;full=relay_keyboard_scaffold_draws;
    for(int i=0;i<8;i++){tap(x+w/2,y+h/2);assert(render_changed(b,0));}
    assert(!strcmp(relay_edit.password,"aaaaaaaa")&&relay_keyboard_scaffold_draws==full&&relay_keyboard_dynamic_draws==dynamic+8);
    relay_render_reference(b,reference);relay_cached_press(b,"rclear",0,reference);
    relay_cached_press(b,"rk61",0,reference);relay_cached_press(b,"rk61",0,reference);
    relay_cached_press(b,"rshift",1,reference);relay_cached_press(b,"rk41",0,reference);
    relay_cached_press(b,"rshift",1,reference);relay_cached_press(b,"rk61",0,reference);
    relay_cached_press(b,"rsymbol",1,reference);relay_cached_press(b,"rk22",0,reference);relay_cached_press(b,"rk27",0,reference);
    relay_cached_press(b,"rsymbol",1,reference);relay_cached_press(b,"rk5c",0,reference);relay_cached_press(b,"rspace",0,reference);
    assert(!strcmp(relay_edit.password,"aaAa\"'\\ "));
    char display[35]={0},count[64]={0};relay_keyboard_display(display,count);
    assert(!strcmp(display,"********")&&!strstr(html,"aaAa")&&!strstr(drawn_html,"aaAa"));
    relay_cached_press(b,"rshow",1,reference);relay_keyboard_display(display,count);
    assert(!strcmp(display,relay_edit.password)&&!strstr(html,"aaAa")&&!strstr(drawn_html,"aaAa"));
    relay_cached_press(b,"rshow",1,reference);relay_cached_press(b,"rbs",0,reference);relay_cached_press(b,"rclear",0,reference);
    /* Repeated identical coordinates and a long tail: key hit doc survives all edits. */
    for(int i=0;i<64;i++)relay_cached_press(b,"rk31",0,reference);
    relay_cached_press(b,"rshow",1,reference);relay_keyboard_display(display,count);
    assert(strlen(display)>0&&strlen(display)<=34&&strspn(display,"1")==strlen(display)&&strlen(relay_edit.password)==64);
    assert(relay_keyboard_text_extent(display,NULL)<=296);
    relay_cached_press(b,"rshow",1,reference);relay_cached_press(b,"rk31",0,reference);
    assert(relay_edit.overflow);relay_cached_press(b,"rbs",0,reference);assert(!relay_edit.overflow);
    relay_cached_press(b,"rbs",0,reference);relay_cached_press(b,"rclear",0,reference);assert(!relay_edit.password[0]);
    relay_cached_press(b,"rshift",1,reference);relay_cached_press(b,"rshow",1,reference);
    for(int i=0;i<32;i++)relay_cached_press(b,"rk57",0,reference);
    relay_keyboard_display(display,count);assert(strspn(display,"W")==strlen(display)&&relay_keyboard_text_extent(display,NULL)<=296);
    relay_cached_press(b,"rclear",0,reference);relay_cached_press(b,"rshow",1,reference);
    relay_cached_press(b,"rletters",1,reference);relay_cached_press(b,"rk61",0,reference);
    /* Language/dialog transitions must discard the cached scaffold, not the hit path. */
    full=relay_keyboard_scaffold_draws;settings_zh=!lang;
    assert(render_changed(b,0)&&relay_keyboard_scaffold_draws==full+1);relay_render_reference(b,reference);
    settings_zh=lang;assert(render_changed(b,0));relay_render_reference(b,reference);
    dialog=3;snprintf(notice,sizeof(notice),"Synthetic render test");assert(render_changed(b,0)&&!relay_keyboard_cache.valid);
    dialog=0;full=relay_keyboard_scaffold_draws;
    assert(render_changed(b,0)&&relay_keyboard_scaffold_draws==full+1);relay_render_reference(b,reference);
    relay_cached_press(b,"rshow",1,reference);relay_cached_press(b,"rkcancel",-1,reference);
    for(size_t i=0;i<sizeof(relay_edit.password);i++)assert(!relay_edit.password[i]);
    for(size_t i=0;i<sizeof(relay_edit.before);i++)assert(!relay_edit.before[i]);
    assert(!relay_keyboard_cache.valid&&!relay_edit.show);
    /* SSID is always visible; entering another field requires a new scaffold. */
    relay_cached_press(b,"rssid",1,reference);relay_cached_press(b,"rk78",0,reference);
    relay_keyboard_display(display,count);assert(!strcmp(display,"x"));
    relay_cached_press(b,"rspace",0,reference);relay_cached_press(b,"rbs",0,reference);
    relay_cached_press(b,"rkdone",-1,reference);assert(!strcmp(relay_edit.ssid,"x"));
    relay_cached_press(b,"rssid",1,reference);relay_cached_press(b,"rk79",0,reference);
    relay_cached_press(b,"rkcancel",-1,reference);assert(!strcmp(relay_edit.ssid,"x"));
    /* Leaving the page or power must wipe editor and repaint the old text pixels. */
    relay_cached_press(b,"rpassword",1,reference);relay_cached_press(b,"rk61",0,reference);relay_cached_press(b,"rshow",1,reference);
    power_menu();assert(!relay_edit.active&&!relay_edit.password[0]);assert(render_changed(b,0)&&!relay_keyboard_cache.valid);
    relay_render_reference(b,reference);dialog=0;page=MENU;assert(render_changed(b,0));relay_render_reference(b,reference);
    relay_wipe(display,sizeof(display));relay_wipe(count,sizeof(count));
  }
  settings_zh=zh;relay_editor_clear();relay_view=RV_STATUS;relay_wipe(reference,W*H*2);free(reference);
  puts("PASS cached keyboard render: real repeated HTML hits, text-only edits, structural rebuilds, ASCII/tail/mask/bounds/cancel/power wipes, both languages, full-frame pixel equivalence; no IPC");
}
static void relay_full_tests(struct drm_buf *b,const char *dir) {
  struct snapshot old=data;int old_menu=settings_menu;char fixture[4096],changed[4096],copy[4096];
  char ssid[33],hex[67];
  assert(relay_unhex(ssid,"20e4b8ade6968720")&&!strcmp(ssid," 中文 "));
  assert(relay_unhex(ssid,"2020412020")&&!strcmp(ssid,"  A  "));
  const char *const bad_ssids[]={"","0","00","0a","0d","7f","c285","c080","c0af","80","e282","eda080","f0808080","f4908080","f5808080"};
  for(size_t i=0;i<sizeof(bad_ssids)/sizeof(bad_ssids[0]);i++)assert(!relay_unhex(ssid,bad_ssids[i]));
  for(int i=0;i<33;i++){hex[i*2]='4';hex[i*2+1]='1';}hex[66]=0;assert(!relay_unhex(ssid,hex));hex[64]=0;assert(relay_unhex(ssid,hex)&&strlen(ssid)==32);
  relay_fixture(fixture,sizeof(fixture),21,8);snprintf(copy,sizeof(copy),"%s",fixture);
  assert(parse_snapshot(copy,&data)&&data.relay.has&&data.relay.ns==8&&data.relay.nw==6);
  settings_menu|=1<<UPLINK;page=UPLINK;reset_job();relay_editor_clear();relay_view=RV_STATUS;stub_status=fixture;stub_job="OK 1\n";
  capture_both(b,dir,"relay-full-status");test_log[0]=0;
  relay_press(b,"rsaved");assert(relay_view==RV_SAVED&&!test_log[0]);
  relay_press(b,"rnext");assert(relay_offset==4);relay_press(b,"rprev");assert(!relay_offset);
  capture_both(b,dir,"relay-full-saved");relay_press(b,"rr0");assert(relay_view==RV_DETAIL&&relay_edit.id==1);
  relay_press(b,"rconnect");assert(dialog==5&&confirm_target==RA_CONNECT);capture_both(b,dir,"relay-full-confirm");
  relay_press(b,"confirm");expect_request("urelay connect 21 1\n");assert(pending_kind==JOB_UPLINK&&!relay_edit.active);reset_job();
  relay_press(b,"rreconnect");relay_press(b,"confirm");expect_request("urelay reconnect 21\n");reset_job();
  relay_press(b,"rdisconnect");relay_press(b,"confirm");expect_request("urelay disconnect 21\n");reset_job();
  relay_press(b,"rnearby");assert(relay_view==RV_SCAN);relay_press(b,"rnext");assert(relay_offset==4);
  render(b);assert(strstr(html,"Unsupported")&&strstr(html,"id=\"rr1\" href=\"act:none\""));
  test_request[0]=0;relay_press(b,"rr1");assert(relay_view==RV_SCAN&&!test_request[0]);
  relay_press(b,"rprev");relay_press(b,"rr0");assert(relay_view==RV_EDIT&&relay_edit.from_scan);
  relay_press(b,"rpassword");relay_type(b,"Testpass8");relay_press(b,"rkdone");relay_press(b,"rsave");relay_press(b,"confirm");
  expect_request("urelay save-scan 21 8 0 0 546573747061737338\n");assert(!relay_edit.password[0]);reset_job();
  relay_press(b,"rnearby");relay_press(b,"rscan");assert(dialog==5);relay_press(b,"confirm");expect_request("urelay scan\n");reset_job();relay_view=RV_STATUS;
  relay_press(b,"rmanual");relay_press(b,"rssid");relay_type(b,"Manual Test");relay_press(b,"rkdone");assert(!strcmp(relay_edit.ssid,"Manual Test"));
  relay_press(b,"rpassword");relay_type(b,"aA\"\\ !~9");assert(!strcmp(relay_edit.password,"aA\"\\ !~9"));
  char display[35]={0},count[64]={0};
  render(b);relay_keyboard_display(display,count);assert(!strcmp(display,"********")&&!strstr(html,"aA&quot;"));
  relay_press(b,"rshow");assert(relay_edit.show);render(b);relay_keyboard_display(display,count);
  assert(!strcmp(display,"aA\"\\ !~9")&&!strstr(html,"aA&quot;"));relay_wipe(display,sizeof(display));relay_wipe(count,sizeof(count));
  relay_press(b,"rshow");relay_press(b,"rbs");assert(strlen(relay_edit.password)==7);
  relay_press(b,"rclear");assert(!relay_edit.password[0]);relay_press(b,"rshift");relay_press(b,"rk41");assert(!strcmp(relay_edit.password,"A"));
  relay_press(b,"rshift");relay_press(b,"rk61");assert(!strcmp(relay_edit.password,"Aa"));
  relay_press(b,"rsymbol");relay_press(b,"rk22");relay_press(b,"rk27");relay_press(b,"rsymbol");relay_press(b,"rk5c");relay_press(b,"rspace");
  assert(!strcmp(relay_edit.password,"Aa\"'\\ "));capture_both(b,dir,"relay-keyboard-symbols-masked");
  relay_press(b,"rclear");for(int i=0;i<64;i++)relay_press(b,"rk31");assert(strlen(relay_edit.password)==64);
  relay_press(b,"rk32");assert(relay_edit.overflow&&strlen(relay_edit.password)==64);relay_press(b,"rbs");assert(!relay_edit.overflow&&strlen(relay_edit.password)==63);
  relay_press(b,"rkcancel");assert(relay_view==RV_EDIT&&!relay_edit.password[0]);relay_press(b,"rpassword");relay_type(b,"Mockpass8");relay_press(b,"rkdone");
  capture_both(b,dir,"relay-full-editor");relay_press(b,"rsave");relay_press(b,"confirm");expect_request("urelay save 21 0 4d616e75616c2054657374 wpa2 0 4d6f636b7061737338\n");reset_job();
  relay_press(b,"rsaved");relay_press(b,"rr0");relay_press(b,"redit");assert(relay_edit.keep&&!relay_edit.password[0]);relay_press(b,"rsave");relay_press(b,"confirm");expect_request("urelay save 21 1 4e657430 wpa2 0 -\n");reset_job();
  relay_press(b,"rsaved");relay_press(b,"rr0");relay_press(b,"rdelete");relay_press(b,"confirm");expect_request("urelay delete 21 1\n");reset_job();
  relay_press(b,"rreconnect");relay_fixture(changed,sizeof(changed),22,8);stub_status=changed;test_log[0]=0;relay_press(b,"confirm");
  assert(dialog==3&&!pending&&!strstr(test_log,"urelay ")&&!relay_edit.password[0]);relay_press(b,"refresh");
  snprintf(copy,sizeof(copy),"%s",fixture);assert(parse_snapshot(copy,&data));stub_status=fixture;reset_job();relay_view=RV_STATUS;
  relay_press(b,"rreconnect");data.relay.gen++;render(b);assert(strstr(html,"id=\"confirm\" href=\"act:none\""));dialog_tap("confirm");assert(dialog==3&&!pending);
  reset_job();snprintf(copy,sizeof(copy),"%s",fixture);parse_snapshot(copy,&data);relay_view=RV_SCAN;relay_offset=0;data.stamp=data.relay.stamp+121;assert(!relay_scan_fresh());
  /* Complete printable ASCII coverage of the actual key map, including both symbol pages. */
  for(int c=33;c<=126;c++){int found=0;for(int mode=0;mode<4;mode++){relay_edit.keymode=mode;for(int row=0;row<3;row++)found|=strchr(relay_key_row(row),c)!=NULL;}assert(found);}
  relay_begin();relay_edit.security=1;memset(relay_edit.password,'a',64);relay_edit.password[64]=0;assert(relay_password_valid());relay_edit.security=3;assert(relay_password_valid());relay_edit.security=2;assert(!relay_password_valid());
  relay_edit.key=RK_SSID;memset(relay_edit.ssid,'x',32);relay_edit.ssid[32]=0;relay_key_char('y');assert(relay_edit.overflow&&strlen(relay_edit.ssid)==32);
  relay_edit.key=RK_PASSWORD;power_menu();assert(!relay_edit.password[0]&&!relay_edit.active);dialog=0;
  relay_cached_render_tests(b);
  relay_editor_clear();relay_view=RV_STATUS;stub_status=NULL;data=old;settings_menu=old_menu;page=MENU;reset_job();
  puts("PASS full relay synthetic hit tests: saved/scan paging, QWERTY ASCII, masking, bounds, cancellation, typed save/connect/reconnect/delete, fresh revision recheck, unsupported security and native scan TTL");
}
