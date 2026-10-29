/* SCREEN_PREVIEW only. Wi-Fi page device list, kick (block) and unblock: synthetic I/i
 * records + captured IPC. */
static void wifi_fixture(int stations, int blocked) {
  static char text[4096];unsigned long long now=(unsigned long long)time(NULL);
  size_t used=(size_t)snprintf(text,sizeof(text),"S 7 1 0 0 0 0 0 0 1 %llu\nO 83 320 18372 34.5 12.4\n"
    "H\t-65\t-88\t-10\t19\t5G SA\tN78\tWL_AND_5G\tB1,B3\tN78,N41\t502\t256000\t42000\t12\t35\t34200\t2\t1\t1\t5\tMy_Hotspot\trndis\t0\t64\t1\t1\t80\t80\t70\t4120\t450\tGood\t1\t2\trndis0\t100\t100\t19\t18\n",now);
  static const char *names[]={"iPhone 15","MacBook-Pro","-","客厅电视","Pixel-8","iPad"};
  for(int i=0;i<stations;i++)
    used+=(size_t)snprintf(text+used,sizeof(text)-used,"I\taa:bb:cc:00:00:%02x\t%d\t%d\t%d\t192.168.0.%d\t%s\n",i+1,i%2?2:5,-40-i*5,60+i*700,i+10,names[i%6]);
  for(int i=0;i<blocked;i++)
    used+=(size_t)snprintf(text+used,sizeof(text)-used,"i\tdd:ee:ff:00:00:%02x\t%s\n",i+1,i?"-":"Old TV");
  /* A malformed row is dropped, not shown. */
  used+=(size_t)snprintf(text+used,sizeof(text)-used,"I\tnot-a-mac\t5\t-50\t1\t-\tbad\nJ 0 0 0 0\n.\n");
  assert(parse_snapshot(text,&data));
  reset_job();go(WIFI_PAGE);
}
static void wifi_tests(struct drm_buf *b,const char *dir) {
  wifi_fixture(6,2);
  assert(data.nwsta==6&&data.nwblk==2&&data.wsta[0].band==5&&data.wsta[0].signal==-40&&!strcmp(data.wblk[0].name,"Old TV"));
  capture_both(b,dir,"wifi-main");
  assert(strstr(html,"act:wclients"));
  /* Band switches keep their ids below the device row. */
  press_id(b,"w2");expect_request("tool wifi_toggle 2g 0\n");reset_job();
  /* Power: both bands at 100% mark Max; a level sends both bands' percent. */
  assert(data.toolbox.has_wifi_power&&data.toolbox.wifi_dbm_5g==18&&strstr(html,"18 dBm"));
  press_id(b,"wpw2");expect_request("tool wifi_power 50\n");reset_job();
  press_id(b,"wpw0");assert(!test_request[0]);
  data.toolbox.wifi_pct_5g=50;press_id(b,"wpw0");expect_request("tool wifi_power 100\n");reset_job();
  /* An older agent's H35: power unknown, buttons inert. */
  data.toolbox.has_wifi_power=0;capture_both(b,dir,"wifi-power-unknown");press_id(b,"wpw3");assert(!test_request[0]);
  wifi_fixture(6,2);
  press_id(b,"wclients");assert(wifi_view==1&&wifi_offset==0);capture_both(b,dir,"wifi-devices");
  /* A device without a name shows its MAC; the line shows its address and band. */
  assert(strstr(html,"aa:bb:cc:00:00:03")&&strstr(html,"192.168.0.10 · 5G"));
  press_id(b,"wnext");assert(wifi_offset==4);capture_both(b,dir,"wifi-devices-page2");
  press_id_absent(b,"kick2");press_id(b,"wprev");assert(wifi_offset==0);
  /* Kick asks first and names the device; cancelling sends nothing. */
  press_id(b,"kick0");assert(dialog==5&&confirm_what==CF_KICK&&!test_request[0]&&!strcmp(kick_sta.mac,"aa:bb:cc:00:00:01"));
  capture_both(b,dir,"wifi-kick-confirm");
  press_id(b,"dismiss");assert(!dialog&&!test_request[0]);
  press_id(b,"kick1");press_id(b,"confirm");expect_request("tool wifi_block aa:bb:cc:00:00:02\n");assert(pending_kind==JOB_TOOL);
  /* While it applies, no second kick. */
  test_request[0]=0;press_id(b,"kick0");assert(!dialog&&!test_request[0]);reset_job();
  /* Blocked list: unblock sends at once. */
  press_id(b,"wblocked");assert(wifi_view==2);capture_both(b,dir,"wifi-blocked");
  press_id(b,"unb1");expect_request("tool wifi_unblock dd:ee:ff:00:00:02\n");reset_job();
  /* Back steps out one level at a time. */
  press_id(b,"back");assert(page==WIFI_PAGE&&wifi_view==1);
  press_id(b,"back");assert(page==WIFI_PAGE&&wifi_view==0);
  press_id(b,"back");assert(page!=WIFI_PAGE);
  /* Stale status: rows show, kick does nothing. */
  wifi_fixture(2,0);press_id(b,"wclients");data.stamp-=11;press_id(b,"kick0");assert(!dialog&&!test_request[0]);
  wifi_fixture(0,0);press_id(b,"wclients");capture_both(b,dir,"wifi-devices-empty");
  press_id_absent(b,"kick0");press_id(b,"wblocked");press_id_absent(b,"unb0");
  wifi_fixture(6,2);
  puts("PASS Wi-Fi device list (zh/en): I/i parsing, paging, kick confirmation with exact block command, unblock, back levels, pending/stale guards");
}
