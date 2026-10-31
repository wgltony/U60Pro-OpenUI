/* Offline SCREEN_PREVIEW fixtures only. Repeated digits are synthetic, never device values. */
static void imei_fixture(const char *phase,const char *code,int allowed) {
  char text[1024];unsigned long long now=(unsigned long long)time(NULL);
  snprintf(text,sizeof(text),"S 7 1 0 0 0 0 0 0 1 %llu\nI1\t7\t%llu\t0\t%s\t%s\t111111111111119\t222222222222228\tactive\taaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\tbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\t***********3337\t-\t%d\t60\nJ 0 0 0 0\n.\n",now,now,phase,code,allowed);
  assert(parse_snapshot(text,&data)&&data.imei.has);reset_job();imei_clear();memset(&imei_clock,0,sizeof(imei_clock));imei_observe(&data.imei,monotonic_ms());page=IMEI_PAGE;
  settings_menu=TOOLBOX_MENU_PAGES;stub_job="OK IMEI\n";test_log[0]=0;imei_track();
}
static int imei_record_test(const char **fields,int count,struct imei_info *out) {
  char text[2048]={0};size_t n=0;
  for(int i=0;i<count;i++)n+=(size_t)snprintf(text+n,sizeof(text)-n,"%s%s",i?"\t":"",fields[i]);
  return parse_imei(text,out);
}
static void imei_geometry(void) {
  const char *ids[]={"icheck","irefresh","iedit0","ikeep0","iedit1","ikeep1","ia2","ia4","ia8","ia16","ia32","ia64","ia128","ia256","imore"};
  int r[16][4],count=0;
  for(size_t i=0;i<sizeof(ids)/sizeof(*ids);i++) {
    char selector[24];int x,y,w,h;snprintf(selector,sizeof(selector),"#%s",ids[i]);
    if(!html_view_rect(selector,&x,&y,&w,&h))continue;
    assert(x>=0&&y>=0&&w>=48&&h>=48&&x+w<=W&&y+h<=432);
    for(int j=0;j<count;j++)assert(x>=r[j][0]+r[j][2]||x+w<=r[j][0]||y>=r[j][1]+r[j][3]||y+h<=r[j][1]);
    r[count][0]=x;r[count][1]=y;r[count][2]=w;r[count++][3]=h;
  }
}
static void imei_tests(struct drm_buf *b,const char *dir) {
  struct imei_info info;
  const char *f[]={"I1","7","1","0","idle","not_checked","-","-","unknown","-","-","-","-","1","0","extra"};
  assert(imei_record_test(f,15,&info)&&info.has&&!info.current[0][0]&&!info.op[0]);
  assert(!imei_record_test(f,14,&info)&&!info.has);assert(!imei_record_test(f,16,&info));
  const char *bad[][2]={{"1","0"},{"1","9007199254740992"},{"1","18446744073709551616"},{"1","7x"},{"2","-1"},{"2","0"},{"3","01"},{"3","true"},{"4","new_phase"},{"5","Bad_code"},{"5","<b>"},{"6","11111111111111"},{"6","１１１１１１１１１１１１１１９"},{"8","enabled"},{"9","AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"},{"10","123"},{"11","**********33337"},{"12","***********abcd"},{"13","512"},{"13","1junk"},{"14","61"},{"14","-1"}};
  for(size_t i=0;i<sizeof(bad)/sizeof(*bad);i++){int k=atoi(bad[i][0]);const char *old=f[k];f[k]=bad[i][1];assert(!imei_record_test(f,15,&info)&&!info.has);f[k]=old;}
  assert(imei_luhn("111111111111119")&&!imei_luhn("111111111111118")&&!imei_luhn("000000000000000"));
  char duplicate[]="S 7 1 0 0 0 0 0 0 1 1\nI1\t7\t1\t0\tidle\tnot_checked\t-\t-\tunknown\t-\t-\t-\t-\t1\t0\nI1\t7\t1\t0\tidle\tnot_checked\t-\t-\tunknown\t-\t-\t-\t-\t1\t0\n.\n";
  struct snapshot snap;assert(parse_snapshot(duplicate,&snap)&&!snap.imei.has);
  char longcode[600];memset(longcode,'a',sizeof(longcode)-1);longcode[sizeof(longcode)-1]=0;f[5]=longcode;assert(!imei_record_test(f,15,&info));f[5]="not_checked";
  imei_fixture("idle","not_checked",IA_CHECK);data.imei.current[0][0]=data.imei.current[1][0]=0;
  page=SIGNAL_PAGE;capture_both(b,dir,"imei-signal-entry");assert(!test_request[0]);press_id(b,"imei");assert(page==IMEI_PAGE&&!test_request[0]);
  capture_both(b,dir,"imei-not-checked");assert(!test_request[0]);press_id(b,"icheck");expect_request("uimei check 7\n");
  assert(imei_ui.sent==7&&!pending);char sent[8192];snprintf(sent,sizeof(sent),"%s",test_log);imei_tap("icheck");assert(!strcmp(sent,test_log));
  data.imei.rev=8;imei_track();assert(!imei_ui.sent);
  imei_fixture("checked","ready",IA_CHECK|IA_PREVIEW);capture_both(b,dir,"imei-checked");imei_geometry();
  press_id(b,"iedit0");assert(imei_ui.key==0&&!imei_ui.input[0]);capture_both(b,dir,"imei-numeric-editor");
  const char *target="333333333333337";
  for(const char *p=target;*p;p++){char id[8];snprintf(id,sizeof(id),"ik%c",*p);press_id(b,id);}
  press_id(b,"ikok");assert(imei_ui.key==-1&&!strcmp(imei_ui.target[0],target)&&!test_request[0]);
  assert(imei_targets_valid());press_id(b,"ia2");expect_request("uimei preview 7 333333333333337 -\n");
  /* Restore default carries no targets of its own: one tap queues a preview, never a write. */
  imei_fixture("checked","ready",IA_CHECK|IA_PREVIEW|IA_DEFAULT);capture_both(b,dir,"imei-restore-default");imei_geometry();
  press_id(b,"ia256");expect_request("uimei restore_default 7\n");assert(!imei_ui.stage&&imei_ui.sent==7);
  imei_fixture("checked","ready",IA_DEFAULT);data.imei.current[0][14]='8';assert(!imei_can(IA_DEFAULT));imei_tap("ia256");assert(!test_request[0]);
  imei_fixture("checked","ready",IA_PREVIEW);imei_tap("ia256");assert(!test_request[0]);
  imei_fixture("checked","ready",IA_PREVIEW);press_id(b,"iedit0");press_id(b,"ik3");press_id(b,"ikbs");assert(!imei_ui.input[0]);press_id(b,"ik4");press_id(b,"ikclear");assert(!imei_ui.input[0]);press_id(b,"ikcancel");assert(!imei_ui.input[0]&&!test_request[0]);
  snprintf(imei_ui.target[0],16,"%s",data.imei.current[0]);assert(!imei_targets_valid());
  snprintf(imei_ui.target[0],16,"%s",data.imei.current[1]);assert(!imei_targets_valid());
  const char *pairs[][2]={{"222222222222228","111111111111119"},{"222222222222228","333333333333337"},{"333333333333337","111111111111119"},{"","111111111111119"}};
  for(size_t i=0;i<sizeof(pairs)/sizeof(*pairs);i++) {
    snprintf(imei_ui.target[0],16,"%s",pairs[i][0]);snprintf(imei_ui.target[1],16,"%s",pairs[i][1]);
    assert(!imei_targets_valid());imei_tap("ia2");assert(!test_request[0]);
  }
  snprintf(imei_ui.target[0],16,"%s",data.imei.current[0]);snprintf(imei_ui.target[1],16,"%s",target);assert(imei_targets_valid());
  snprintf(imei_ui.target[0],16,"%s",target);snprintf(imei_ui.target[1],16,"%s",data.imei.current[1]);assert(imei_targets_valid());
  snprintf(data.imei.secondary,sizeof(data.imei.secondary),"inactive");snprintf(imei_ui.target[1],16,"%s",target);assert(!imei_targets_valid());imei_track();assert(!imei_ui.target[1][0]);
  capture_both(b,dir,"imei-inactive-secondary");press_id_absent(b,"iedit1");
  imei_fixture("preview","preview_ready",IA_BEGIN|IA_CANCEL);capture_both(b,dir,"imei-preview");
  press_id(b,"ia4");assert(imei_ui.stage==1&&!imei_ui.risk&&!test_request[0]);capture_both(b,dir,"imei-risk-confirm");
  imei_tap("isend");assert(!test_request[0]);press_id(b,"inext");assert(imei_ui.stage==1&&!test_request[0]);
  press_id(b,"irisk");press_id(b,"inext");assert(imei_ui.stage==2&&!test_request[0]);capture_both(b,dir,"imei-second-confirm");
  press_id(b,"isend");expect_request("uimei begin 7 bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb 1\n");
  snprintf(sent,sizeof(sent),"%s",test_log);imei_tap("isend");assert(!strcmp(sent,test_log));
  imei_fixture("preview","preview_ready",IA_BEGIN);press_id(b,"ia4");press_id(b,"iabort");assert(!imei_ui.stage&&!test_request[0]);
  for(int mode=0;mode<6;mode++) {
    imei_fixture("preview","preview_ready",IA_BEGIN);press_id(b,"ia4");press_id(b,"irisk");press_id(b,"inext");
    if(mode==0)data.imei.rev++;else if(mode==1){imei_clock.started-=11000;imei_clock.received-=11000;}else if(mode==2)data.imei.busy=1;else if(mode==3)data.imei.preview[0]='c';else if(mode==4)data.imei.expires=0;else snprintf(data.imei.code,sizeof(data.imei.code),"unknown_safe_code");
    imei_tap("isend");assert(!test_request[0]);imei_track();assert(!imei_ui.stage);
  }
  const int actions[]={IA_CONTINUE,IA_VERIFY,IA_RESTORE,IA_REBOOT,IA_CANCEL};
  const char *names[]={"continue","verify","restore","reboot","cancel"};
  for(int i=0;i<5;i++) {
    imei_fixture(i==0?"ready_to_write":i==1?"reboot_to_verify":"recovery_required",i==0?"awaiting_write_confirmation":i==1?"awaiting_final_reboot":"recovery_required",actions[i]);
    char id[16],want[128];snprintf(id,sizeof(id),"ia%d",actions[i]);
    capture_both(b,dir,i==0?"imei-continue":i==1?"imei-verify":i==2?"imei-recovery":i==3?"imei-reboot":"imei-cancel");
    press_id(b,id);if(actions[i]!=IA_VERIFY){assert(!test_request[0]);press_id(b,"irisk");press_id(b,"inext");press_id(b,"isend");}
    snprintf(want,sizeof(want),"uimei %s 7 aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa%s\n",names[i],actions[i]==IA_VERIFY?"":" 1");expect_request(want);
  }
  imei_fixture("unsupported","helpers_unavailable",255);capture_both(b,dir,"imei-unavailable");assert(!imei_can(IA_CHECK)&&!imei_can(IA_BEGIN));
  imei_fixture("checked","unknown_safe_code",255);assert(imei_can(IA_CHECK)&&imei_can(IA_VERIFY)&&!imei_can(IA_PREVIEW)&&!imei_can(IA_RESTORE));
  const unsigned long long offsets[]={1,9007199254740991ULL};
  for(size_t i=0;i<sizeof(offsets)/sizeof(*offsets);i++) {
    imei_fixture("recovery_required","recovery_required",255);data.stamp=data.imei.stamp=offsets[i];
    assert(imei_can(IA_CHECK)&&imei_can(IA_VERIFY)&&imei_can(IA_RESTORE)&&imei_can(IA_BEGIN));
    imei_tap("ia4");imei_tap("irisk");imei_tap("inext");data.stamp=data.imei.stamp=offsets[1-i];imei_track();
    assert(imei_ui.stage==2&&imei_consent_valid()&&!test_request[0]);
  }
  imei_fixture("checked","ready",255);imei_observe(&data.imei,monotonic_ms()-10001);assert(!imei_can(IA_CHECK));imei_tap("icheck");assert(!test_request[0]);
  imei_fixture("checked","ready",255);imei_clock.started-=10001;imei_clock.received-=10001;assert(!imei_can(IA_CHECK));
  imei_fixture("checked","ready",255);imei_observe(&data.imei,monotonic_ms()-9000);assert(imei_can(IA_CHECK));imei_clock.started-=1001;assert(!imei_can(IA_CHECK));
  imei_fixture("checked","ready",255);imei_observe(&data.imei,monotonic_ms()+1000);assert(!imei_can(IA_CHECK));
  imei_fixture("preview","preview_ready",255);data.imei.expires=1;imei_observe(&data.imei,monotonic_ms()-1500);
  assert(imei_can(IA_CHECK)&&!imei_can(IA_BEGIN));imei_tap("ia4");assert(!imei_ui.stage&&!test_request[0]);
  imei_fixture("preview","preview_ready",255);imei_tap("ia4");imei_tap("irisk");imei_tap("inext");
  imei_clock.preview_until=monotonic_ms()-1;imei_observe(&data.imei,monotonic_ms());imei_track();assert(!imei_ui.stage&&!imei_can(IA_BEGIN));
  data.imei.has=0;imei_observe(&data.imei,monotonic_ms());data.imei.has=1;imei_observe(&data.imei,monotonic_ms());
  assert(!imei_can(IA_BEGIN));imei_tap("isend");assert(!test_request[0]);
  data.imei.rev++;memset(data.imei.preview,'c',32);imei_observe(&data.imei,monotonic_ms());assert(imei_can(IA_BEGIN)&&!test_request[0]);
  imei_fixture("checked","ready",255);data.imei.rev=6;imei_track();assert(!imei_can(IA_CHECK));
  imei_fixture("preview","preview_ready",255);data.imei.preview[0]=0;data.imei.op[0]=0;assert(!imei_can(IA_BEGIN)&&!imei_can(IA_VERIFY)&&!imei_can(IA_RESTORE));
  imei_fixture("checked","ready",255);data.imei.busy=1;assert(!imei_can(IA_CHECK)&&!imei_can(IA_PREVIEW));
  imei_fixture("checked","ready",255);data.imei.has=0;assert(!imei_can(IA_CHECK));
  imei_fixture("checked","ready",255);data.imei.current[0][14]='8';capture_both(b,dir,"imei-invalid-original");assert(!imei_can(IA_PREVIEW)&&!imei_can(IA_BEGIN)&&!imei_can(IA_CONTINUE));
  imei_fixture("checked","ready",511);for(int i=0;i<3;i++){capture_both(b,dir,i==0?"imei-many-actions-1":i==1?"imei-many-actions-2":"imei-many-actions-3");imei_geometry();press_id(b,"imore");}
  imei_fixture("idle","not_checked",IA_CHECK);stub_job="OK 7\n";press_id(b,"icheck");assert(imei_ui.uncertain&&!imei_ui.sent&&imei_ui.error[0]);
  snprintf(sent,sizeof(sent),"%s",test_log);imei_tap("icheck");assert(!strcmp(sent,test_log));capture_both(b,dir,"imei-outcome-unknown");
  imei_fixture("checked","ready",IA_PREVIEW);snprintf(imei_ui.target[0],16,"%s",target);press_id(b,"iedit0");go(SIGNAL_PAGE);assert(!imei_ui.target[0][0]&&!imei_ui.input[0]&&imei_ui.key==-1);
  go(IMEI_PAGE);capture_both(b,dir,"imei-reopened");assert(!imei_ui.target[0][0]&&!test_request[0]);
  snprintf(imei_ui.target[0],16,"%s",target);snprintf(data.imei.phase,sizeof(data.imei.phase),"complete");imei_track();assert(!imei_ui.target[0][0]);
  imei_clear();page=MENU;stub_job="OK 1\n";
  puts("PASS IMEI strict I1/15 fields/duplicates/overflow, 320x480 zh/en fixtures, explicit CHECK only, restore default as a preview, numeric local-only OK, exact typed daemon commands, keep/Luhn/topology/cross-original collision, two consents, revisions/tokens/monotonic expiry, +/- epoch offsets, delayed responses/local age, polling cannot renew tokens, stale/busy/unavailable, no replay and unmount clearing (synthetic only)");
}
