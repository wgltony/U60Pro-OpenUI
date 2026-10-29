/* Full relay workflow, only advertised with a complete L/l/w catalogue. */
enum { RA_SCAN=10, RA_SAVE, RA_CONNECT, RA_RECONNECT, RA_DISCONNECT, RA_DELETE };
static const char *relay_security_name(int s) {
  static const char *const names[]={"Open","WPA2","WPA3 SAE","WPA2 / WPA3","Unsupported"};
  return s>=0&&s<=4?names[s]:"Unsupported";
}
static const char *relay_security_wire(int s) {
  static const char *const names[]={"open","wpa2","wpa3","wpa2_wpa3"};return s>=0&&s<4?names[s]:"unsupported";
}
static int relay_ready(void) { return data.relay.has&&data.uplink.has&&!blocked()&&data.relay.rev==data.uplink.rev; }
static int relay_scan_fresh(void) {
  return relay_ready()&&data.relay.state==2&&data.relay.gen&&data.relay.stamp&&
    data.stamp>=data.relay.stamp&&data.stamp-data.relay.stamp<=120;
}
static int relay_form_valid(void) {
  return relay_edit.active&&relay_edit.ssid[0]&&relay_edit.security>=0&&relay_edit.security<4&&relay_password_valid()&&
    (!relay_edit.from_scan||(relay_scan_fresh()&&relay_edit.gen==data.relay.gen));
}
static int relay_confirm_valid(void) {
  if(!relay_ready()||confirm_rev!=data.uplink.rev||relay_edit.gen!=data.relay.gen)return 0;
  if(confirm_target==RA_SAVE)return relay_form_valid()&&relay_edit.rev==data.uplink.rev;
  if(confirm_target==RA_CONNECT||confirm_target==RA_DELETE) {
    for(int i=0;i<data.relay.ns;i++)if(data.relay.saved[i].id==relay_edit.id)return 1;
    return 0;
  }
  return confirm_target==RA_SCAN||confirm_target==RA_RECONNECT||(confirm_target==RA_DISCONNECT&&data.uplink.enabled);
}
static void relay_warning(int y) {
  footer(y,40,L("Shared radio: hotspot / AP clients may disconnect.","共享射频：热点 / AP 客户端可能断开。"),"warn");
}
static void render_relay_form(void) {
  header(L("Network editor","编辑网络"));
  nav_row(12,72,296,52,"rssid",L("SSID","网络名称"),relay_edit.ssid[0]?relay_edit.ssid:L("Tap to enter ASCII SSID","点击输入 ASCII SSID"),"",!relay_edit.from_scan&&relay_ascii(relay_edit.ssid));
  nav_row(12,132,296,52,"rsecurity",L("Security (tap to cycle)","安全类型（点击切换）"),relay_security_name(relay_edit.security),"",!relay_edit.from_scan);
  switch_row(12,192,296,48,"rhidden","rhidden",L("Hidden network","隐藏网络"),L("Manual SSID is supported","支持手动输入 SSID"),relay_edit.hidden, !relay_edit.from_scan);
  const char *hint=relay_edit.security==0?L("Open network: no password","开放网络：无需密码"):relay_edit.keep?L("Keep saved password","保留已保存密码"):relay_edit.password[0]?L("New password entered (masked)","已输入新密码（隐藏）"):L("Enter 8–63 ASCII; WPA2 hex64","输入 8–63 ASCII；WPA2 支持 hex64");
  nav_row(12,248,296,52,"rpassword",L("Password","密码"),hint,"",relay_edit.security>0&&relay_edit.security<4);
  linkbox(12,308,142,44,"rkeep",L("Keep saved","保留原密码"),relay_edit.saved&&relay_edit.has_secret&&relay_edit.security>0&&relay_edit.security==relay_edit.original_security,relay_edit.keep?CHOSEN:PLAIN);
  linkbox(166,308,142,44,"rclearpass",L("Clear password","清空密码"),relay_edit.security>0,PLAIN);
  relay_warning(360);
  linkbox(12,414,142,52,"rcancel",L("Cancel","取消"),1,PLAIN);
  linkbox(166,414,142,52,"rsave",L("Save only","仅保存"),relay_ready()&&relay_edit.rev==data.uplink.rev&&relay_form_valid(),PRIMARY);
}
static void render_relay_detail(void) {
  header(L("Saved network","已保存网络"));
  sbox(16,78,288,24,"head",relay_edit.ssid);
  box(16,110,288,20,"muted",relay_security_name(relay_edit.security));
  box(16,140,288,44,"wrap muted",L("Preferred is not connected. Check live status / IP.","优先不等于已连接，请查看实时状态 / IP。"));
  linkbox(12,202,296,52,"rconnect",L("Connect / switch to this network","连接 / 切换到此网络"),relay_ready(),PRIMARY);
  linkbox(12,266,296,52,"redit",L("Edit network","编辑网络"),relay_ready(),PLAIN);
  linkbox(12,330,296,52,"rdelete",L("Delete saved network","删除已保存网络"),relay_ready(),DANGER);
  relay_warning(402);
}
static void render_relay_list(void) {
  int scan=relay_view==RV_SCAN;const struct relay_catalog *r=&data.relay;
  header(scan?L("Nearby Wi-Fi","附近 Wi-Fi"):L("Saved networks","已保存网络"));
  char tmp[128];int total=scan?r->nw:r->ns;
  snprintf(tmp,sizeof(tmp),L("%d networks%s","%d 个网络%s"),total,scan?(r->state==1?L(" · scanning"," · 扫描中"):r->state==3?L(" · scan failed"," · 扫描失败"):!relay_scan_fresh()?L(" · expired / not scanned"," · 过期 / 未扫描"):""):"");
  sbox(12,70,296,24,"muted",tmp);
  for(int i=0;i<4&&relay_offset+i<total;i++) {
    int j=relay_offset+i;char id[12];snprintf(id,sizeof(id),"rr%d",i);
    const char *ssid=scan?r->scan[j].ssid:r->saved[j].ssid;int sec=scan?r->scan[j].security:r->saved[j].security;
    if(scan)snprintf(tmp,sizeof(tmp),"%d dBm · %s · %d MHz",r->scan[j].signal,relay_security_name(sec),r->scan[j].frequency);
    else snprintf(tmp,sizeof(tmp),"%s%s",relay_security_name(sec),r->saved[j].preferred?L(" · preferred"," · 优先"):"");
    nav_row(12,102+i*58,296,52,id,ssid,tmp,sec==4?"bad":"",relay_ready()&&(!scan||(relay_scan_fresh()&&sec<4)));
  }
  if(!total)box(16,136,288,60,"wrap muted",scan?L("Tap Scan. Nothing runs automatically. Unsupported security is read-only.","点击扫描，不会自动扫描。不支持的安全类型仅供查看。"):L("No saved networks. Add a manual SSID or scan nearby Wi-Fi.","尚无已保存网络。可手动添加 SSID 或扫描附近 Wi-Fi。"));
  linkbox(12,342,92,44,"rprev",L("Previous","上一页"),relay_offset>0,PLAIN);
  snprintf(tmp,sizeof(tmp),"%d / %d",total?relay_offset/4+1:0,(total+3)/4);box(112,354,96,20,"center muted",tmp);
  linkbox(216,342,92,44,"rnext",L("Next","下一页"),relay_offset+4<total,PLAIN);
  linkbox(12,396,142,48,"rmanual",L("Manual SSID","手动添加"),relay_ready(),PLAIN);
  linkbox(166,396,142,48,"rscan",L("Scan…","扫描…"),relay_ready(),PRIMARY);
  sbox(12,450,296,20,"warn",L("Shared radio: hotspot clients may disconnect","共享射频：热点客户端可能断开"));
}
static int render_relay_full(void) {
  if(!data.relay.has&&relay_view==RV_STATUS)return 0;
  if(relay_view==RV_KEYBOARD){render_relay_keyboard();return 1;}
  if(relay_view==RV_EDIT){render_relay_form();return 1;}
  if(relay_view==RV_DETAIL){render_relay_detail();return 1;}
  if(relay_view==RV_SAVED||relay_view==RV_SCAN){render_relay_list();return 1;}
  return 0;
}
