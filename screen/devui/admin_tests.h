/* SCREEN_PREVIEW only. Synthetic state + captured IPC, no live service operations. */
static void admin_fixture(void) {
  char text[1024];unsigned long long now=(unsigned long long)time(NULL);
  snprintf(text,sizeof(text),"S 7 1 0 0 0 0 0 0 1 %llu\nO 83 320 18372 34.5 12.4\nA\t1\t0\t%llu\t0\t1\t1\t1\t1\t0\t1\t1\nJ 0 0 0 0\n.\n",now,now);
  assert(parse_snapshot(text,&data));assert(data.admin.has&&data.admin.rev==0);
  reset_job();page=OVERVIEW;
}
static void admin_tests(struct drm_buf *b,const char *dir) {
  struct admin_info a;
  char *fields[]={"A","1","0","1","0","1","1","0","1","0","1","1"};
  parse_admin(fields,12,&a);assert(a.has&&a.rev==0&&a.adb_running&&!a.adb_boot);
  for(int i=4;i<12;i++) {
    char *saved=fields[i];fields[i]="false";parse_admin(fields,12,&a);assert(!a.has);fields[i]=saved;
  }
  fields[2]="18446744073709551616";parse_admin(fields,12,&a);assert(!a.has);fields[2]="0";
  fields[3]="-1";parse_admin(fields,12,&a);assert(!a.has);fields[3]="1";
  fields[1]="2";parse_admin(fields,12,&a);assert(!a.has);fields[1]="1";
  parse_admin(fields,11,&a);assert(!a.has);parse_admin(fields,13,&a);assert(!a.has);
  char missing[]="S 7 1 0 0 0 0 0 0 1 1\n.\n";struct snapshot s;
  assert(parse_snapshot(missing,&s)&&!s.admin.has);
  admin_fixture();capture_both(b,dir,"admin-overview");
  assert(!test_request[0]&&!strstr(html,"act:proxy")&&!strstr(html,"act:uplink"));
  int previous_bottom=0;
  for(int i=0;i<4;i++) {
    char selector[16];int x,y,w,h;snprintf(selector,sizeof(selector),"#admin%d",i);
    assert(html_view_rect(selector,&x,&y,&w,&h));
    /* The layout engine offsets bordered hit rectangles by 1px; stay above the y=460 status note. */
    int fits=x>=0&&y>=0&&y>=previous_bottom&&w>=44&&h>=48&&x+w<=W&&y+h<=H&&y+h<=460;
    if(!fits)fprintf(stderr,"admin geometry: selector=%s x=%d y=%d w=%d h=%d previous_bottom=%d status_start=460 canvas=%dx%d\n",selector,x,y,w,h,previous_bottom,W,H);
    assert(fits);previous_bottom=y+h;
  }
  /* Capture revision + target, but never submit before explicit confirmation. */
  press_id(b,"admin0");assert(dialog==5&&confirm_what==CF_ADMIN&&confirm_rev==0&&!test_request[0]);
  capture_both(b,dir,"admin-runtime-off-confirm");press_id(b,"dismiss");assert(!test_request[0]);
  press_id(b,"admin0");press_id(b,"confirm");expect_request("admin 0 adb runtime 0\n");assert(pending_kind==JOB_ADMIN);
  char sent[128];snprintf(sent,sizeof(sent),"%s",test_request);dialog_tap("confirm");assert(!strcmp(sent,test_request));
  admin_fixture();press_id(b,"admin1");press_id(b,"confirm");expect_request("admin 0 adb boot 0\n");
  admin_fixture();press_id(b,"admin3");press_id(b,"confirm");expect_request("admin 0 ssh boot 1\n");
  admin_fixture();data.admin.ssh_running=0;press_id(b,"admin2");capture_both(b,dir,"admin-runtime-on-confirm");
  press_id(b,"confirm");expect_request("admin 0 ssh runtime 1\n");
  admin_fixture();data.admin.adb_running=0;press_id(b,"admin2");capture_both(b,dir,"admin-last-channel-confirm");
  /* Any competing revision, stale timestamp or busy transition cancels eligibility. */
  admin_fixture();press_id(b,"admin0");data.admin.rev++;press_id(b,"confirm");assert(!test_request[0]);
  admin_fixture();press_id(b,"admin0");data.admin.stamp-=11;press_id(b,"confirm");assert(!test_request[0]);
  admin_fixture();press_id(b,"admin0");data.admin.busy=1;press_id(b,"confirm");assert(!test_request[0]);
  admin_fixture();data.admin.stamp-=11;capture_both(b,dir,"admin-stale");press_id(b,"admin0");assert(!dialog&&!test_request[0]);
  admin_fixture();data.admin.stamp+=30;assert(!admin_can(0));
  admin_fixture();data.admin.has=0;capture_both(b,dir,"admin-unknown");press_id(b,"admin0");assert(!dialog&&!test_request[0]);
  admin_fixture();data.admin.adb_available=0;data.admin.ssh_available=0;capture_both(b,dir,"admin-unavailable");
  press_id(b,"admin0");press_id(b,"admin3");assert(!dialog&&!test_request[0]);
  admin_fixture();data.admin.adb_exposed=0;data.admin.adb_running=0;capture_both(b,dir,"admin-no-usb");
  assert(!admin_can(0));press_id(b,"admin0");assert(!dialog&&!test_request[0]);
  data.admin.adb_running=1;assert(admin_can(0));capture_both(b,dir,"admin-daemon-only");
  admin_fixture();data.admin.busy=1;capture_both(b,dir,"admin-busy");assert(!admin_can(0));
  admin_fixture();
  puts("PASS admin A12 strict parsing, four 48px targets (zh/en), exact runtime/boot commands, confirmations, stale/revision/busy/double-send guards, no automatic actions and daemon/exposure distinction");
}
