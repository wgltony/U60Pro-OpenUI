/* Typed urelay commands. No shell quoting; only IDs, strict enums and hex bytes. */
static void relay_hex(char *out,const char *s) {
  static const char hex[]="0123456789abcdef";
  for(;*s;s++){unsigned c=(unsigned char)*s;*out++=hex[c>>4];*out++=hex[c&15];}*out=0;
}
static void relay_begin(void) {
  relay_editor_clear();relay_edit.active=1;relay_edit.rev=data.uplink.rev;relay_edit.gen=data.relay.gen;
}
static void relay_choose_saved(int i) {
  const struct relay_saved *s=&data.relay.saved[i];relay_begin();
  relay_edit.id=s->id;relay_edit.saved=1;relay_edit.keep=s->has_password;
  relay_edit.security=s->security;relay_edit.original_security=s->security;relay_edit.has_secret=s->has_password;relay_edit.hidden=s->hidden;
  snprintf(relay_edit.ssid,sizeof(relay_edit.ssid),"%s",s->ssid);relay_view=RV_DETAIL;
}
static void relay_ask(int action) {
  /* Even non-scan actions capture the scan generation: a scan changes the catalogue. */
  relay_edit.gen=data.relay.gen;
  ask(CF_RELAY,action,JOB_UPLINK,data.uplink.rev,"");
}
static void relay_invalidate(void) {
  relay_editor_clear();relay_view=RV_STATUS;dialog=3;failure_text[0]=0;
  snprintf(notice,sizeof(notice),"%s",L("Relay state changed or expired. Refresh and choose again.","中继状态已变化或过期，请刷新后重新选择。"));need_draw=1;
}
static int relay_refresh_before_send(void) {
  char reply[49152];struct snapshot fresh;
  if(exchange("status\n",reply,sizeof(reply))||!parse_snapshot(reply,&fresh))return 0;
  data=fresh;update_settings(&data);need_draw=1;
  return relay_confirm_valid();
}
static void relay_confirm_send(void) {
  if(!relay_confirm_valid()||!relay_refresh_before_send()){relay_invalidate();return;}
  char req[384]="",ssid_hex[65]="",password_hex[129]="";
  unsigned long long rev=confirm_rev;int action=confirm_target;
  if(action==RA_SCAN)snprintf(req,sizeof(req),"urelay scan\n");
  else if(action==RA_SAVE) {
    relay_hex(ssid_hex,relay_edit.ssid);
    if(!relay_edit.security||relay_edit.keep)strcpy(password_hex,"-");else relay_hex(password_hex,relay_edit.password);
    if(relay_edit.from_scan)snprintf(req,sizeof(req),"urelay save-scan %llu %llu %d %llu %s\n",rev,relay_edit.gen,relay_edit.scan_index,relay_edit.id,password_hex);
    else snprintf(req,sizeof(req),"urelay save %llu %llu %s %s %d %s\n",rev,relay_edit.id,ssid_hex,relay_security_wire(relay_edit.security),relay_edit.hidden,password_hex);
  } else if(action==RA_CONNECT||action==RA_DELETE)snprintf(req,sizeof(req),"urelay %s %llu %llu\n",action==RA_CONNECT?"connect":"delete",rev,relay_edit.id);
  else snprintf(req,sizeof(req),"urelay %s %llu\n",action==RA_RECONNECT?"reconnect":"disconnect",rev);
  send_job(req,JOB_UPLINK,rev);
  relay_wipe(req,sizeof(req));relay_wipe(password_hex,sizeof(password_hex));relay_wipe(ssid_hex,sizeof(ssid_hex));
  relay_editor_clear();relay_view=action==RA_SCAN?RV_SCAN:RV_STATUS;relay_offset=0;
}
static int relay_tap(const char *a) {
  if(page!=UPLINK)return 0;
  if(relay_keyboard_tap(a))return 1;
  if(!strcmp(a,"back")) {
    if(relay_view!=RV_STATUS){relay_editor_clear();relay_view=RV_STATUS;return 1;}return 0;
  }
  if(!data.relay.has)return 0;
  if(!strcmp(a,"rsaved")||!strcmp(a,"rnearby")) {
    relay_editor_clear();relay_view=!strcmp(a,"rsaved")?RV_SAVED:RV_SCAN;relay_offset=0;return 1;
  }
  if(!strcmp(a,"rprev")){if(relay_offset>0)relay_offset-=4;return 1;}
  if(!strcmp(a,"rnext")){int n=relay_view==RV_SCAN?data.relay.nw:data.relay.ns;if(relay_offset+4<n)relay_offset+=4;return 1;}
  if(!strcmp(a,"rcancel")){relay_editor_clear();relay_view=RV_STATUS;return 1;}
  if(!relay_ready())return !strncmp(a,"r",1);
  if(!strcmp(a,"rmanual")){relay_begin();relay_edit.security=1;relay_view=RV_EDIT;return 1;}
  if(!strcmp(a,"rscan")){relay_editor_clear();relay_ask(RA_SCAN);return 1;}
  if(!strcmp(a,"rreconnect")&&data.uplink.saved){relay_editor_clear();relay_ask(RA_RECONNECT);return 1;}
  if(!strcmp(a,"rdisconnect")&&data.uplink.enabled){relay_editor_clear();relay_ask(RA_DISCONNECT);return 1;}
  int i=digit_id(a+1,'r','3');
  if(a[0]=='r'&&i>=0) {
    i+=relay_offset;
    if(relay_view==RV_SAVED&&i<data.relay.ns)relay_choose_saved(i);
    else if(relay_view==RV_SCAN&&relay_scan_fresh()&&i<data.relay.nw&&data.relay.scan[i].security<4) {
      const struct relay_scan *s=&data.relay.scan[i];relay_begin();relay_edit.from_scan=1;relay_edit.scan_index=s->index;relay_edit.security=s->security;
      snprintf(relay_edit.ssid,sizeof(relay_edit.ssid),"%s",s->ssid);relay_view=RV_EDIT;
    }
    return 1;
  }
  if(relay_view==RV_DETAIL) {
    if(!strcmp(a,"rconnect"))relay_ask(RA_CONNECT);
    else if(!strcmp(a,"rdelete"))relay_ask(RA_DELETE);
    else if(!strcmp(a,"redit"))relay_view=RV_EDIT;
    return 1;
  }
  if(relay_view==RV_EDIT) {
    if(!strcmp(a,"rssid")&&!relay_edit.from_scan&&relay_ascii(relay_edit.ssid))relay_key_open(RK_SSID);
    else if(!strcmp(a,"rpassword")&&relay_edit.security>0&&relay_edit.security<4)relay_key_open(RK_PASSWORD);
    else if(!strcmp(a,"rsecurity")&&!relay_edit.from_scan) {
      relay_edit.security=(relay_edit.security+1)%4;relay_wipe(relay_edit.password,sizeof(relay_edit.password));relay_edit.keep=0;
    } else if(!strcmp(a,"rhidden")&&!relay_edit.from_scan)relay_edit.hidden=!relay_edit.hidden;
    else if(!strcmp(a,"rkeep")&&relay_edit.saved&&relay_edit.has_secret&&relay_edit.security>0&&relay_edit.security==relay_edit.original_security){relay_wipe(relay_edit.password,sizeof(relay_edit.password));relay_edit.keep=1;}
    else if(!strcmp(a,"rclearpass")){relay_wipe(relay_edit.password,sizeof(relay_edit.password));relay_edit.keep=0;}
    else if(!strcmp(a,"rsave")&&relay_form_valid()&&relay_edit.rev==data.uplink.rev)relay_ask(RA_SAVE);
    return 1;
  }
  return 0;
}
