/* One typed request per deliberate tap; responses only mean queued, never success. */
static void imei_send(int action) {
  if(!imei_can(action)||(action==IA_PREVIEW&&!imei_targets_valid())||
     (action!=IA_CHECK&&action!=IA_PREVIEW&&action!=IA_DEFAULT&&action!=IA_VERIFY&&(!imei_ui.risk||imei_ui.stage!=2||!imei_consent_valid())))return;
  char req[192]={0},reply[128]={0};const struct imei_info *s=&data.imei;
  if(action==IA_CHECK)snprintf(req,sizeof(req),"uimei check %llu\n",s->rev);
  else if(action==IA_PREVIEW)snprintf(req,sizeof(req),"uimei preview %llu %s %s\n",s->rev,imei_ui.target[0][0]?imei_ui.target[0]:"-",imei_ui.target[1][0]?imei_ui.target[1]:"-");
  else if(action==IA_DEFAULT)snprintf(req,sizeof(req),"uimei restore_default %llu\n",s->rev);
  else if(action==IA_VERIFY) {if(!imei_hex(s->op))return;snprintf(req,sizeof(req),"uimei verify %llu %s\n",s->rev,s->op);}
  else snprintf(req,sizeof(req),"uimei %s %llu %s 1\n",imei_action_name(action),imei_ui.rev,imei_ui.token);
  imei_ui.sent=s->rev;imei_ui.stage=0;imei_ui.risk=0;imei_ui.error[0]=0;
  int ok=exchange(req,reply,sizeof(reply))==0&&!strcmp(reply,"OK IMEI\n");
  relay_wipe(req,sizeof(req));relay_wipe(reply,sizeof(reply));relay_wipe(imei_ui.token,sizeof(imei_ui.token));
  if(!ok){imei_ui.sent=0;imei_ui.uncertain=1;snprintf(imei_ui.error,sizeof(imei_ui.error),"%s",L("Outcome unknown / refused. Refresh; NO replay.","结果未知或已拒绝。请刷新，不会自动重发。"));}
}
static void imei_refresh(void) {
  char reply[49152];struct snapshot fresh;long long started=monotonic_ms();
  imei_ui.stage=0;imei_ui.risk=0;
  if(exchange("status\n",reply,sizeof(reply))==0&&parse_snapshot(reply,&fresh)) {
    imei_observe(&fresh.imei,started);
    data=fresh;update_settings(&data);imei_ui.uncertain=0;imei_ui.error[0]=0;imei_track();
  } else {imei_ui.uncertain=1;snprintf(imei_ui.error,sizeof(imei_ui.error),"%s",L("Status unavailable. No request replayed.","状态不可用，未重发任何请求。"));}
  relay_wipe(reply,sizeof(reply));
}
static int imei_tap(const char *a) {
  if(page!=IMEI_PAGE)return 0;
  if(imei_ui.key>=0) {
    if(!strcmp(a,"back")||!strcmp(a,"ikcancel")){relay_wipe(imei_ui.input,sizeof(imei_ui.input));imei_ui.key=-1;return 1;}
    if(!imei_can(IA_PREVIEW)||data.imei.rev!=imei_ui.rev){relay_wipe(imei_ui.input,sizeof(imei_ui.input));imei_ui.key=-1;return 1;}
    size_t n=strlen(imei_ui.input);
    if(!strncmp(a,"ik",2)&&a[2]>='0'&&a[2]<='9'&&!a[3]){if(n<15){imei_ui.input[n]=a[2];imei_ui.input[n+1]=0;}}
    else if(!strcmp(a,"ikbs")){if(n)imei_ui.input[n-1]=0;}
    else if(!strcmp(a,"ikclear"))relay_wipe(imei_ui.input,sizeof(imei_ui.input));
    else if(!strcmp(a,"ikok")&&imei_luhn(imei_ui.input)) {snprintf(imei_ui.target[imei_ui.key],16,"%s",imei_ui.input);relay_wipe(imei_ui.input,sizeof(imei_ui.input));imei_ui.key=-1;}
    return 1;
  }
  if(imei_ui.stage) {
    if(!strcmp(a,"back")||!strcmp(a,"iabort")){imei_ui.stage=0;imei_ui.risk=0;relay_wipe(imei_ui.token,sizeof(imei_ui.token));}
    else if(imei_consent_valid()) {
      if(!strcmp(a,"irisk")&&imei_ui.stage==1)imei_ui.risk=!imei_ui.risk;
      else if(!strcmp(a,"inext")&&imei_ui.risk&&imei_ui.stage==1)imei_ui.stage=2;
      else if(!strcmp(a,"isend")&&imei_ui.risk&&imei_ui.stage==2)imei_send(imei_ui.action);
    }
    return 1;
  }
  if(!strcmp(a,"irefresh")){if(!imei_ui.sent)imei_refresh();return 1;}
  if(!strcmp(a,"imore")){int count=0;for(int bit=IA_PREVIEW;bit<=IA_DEFAULT;bit*=2)if(data.imei.allowed&bit)count++;if(count>4)imei_ui.action_page=(imei_ui.action_page+1)%((count+2)/3);return 1;}
  if(!strcmp(a,"icheck")){imei_send(IA_CHECK);return 1;}
  if(!strcmp(a,"iedit0")||!strcmp(a,"iedit1")) {
    int i=a[5]-'0';if(imei_can(IA_PREVIEW)&&(i==0||!strcmp(data.imei.secondary,"active"))) {
      imei_ui.key=i;imei_ui.rev=data.imei.rev;snprintf(imei_ui.input,sizeof(imei_ui.input),"%s",imei_ui.target[i]);
    }return 1;
  }
  if(!strcmp(a,"ikeep0")||!strcmp(a,"ikeep1")){if(imei_can(IA_PREVIEW))relay_wipe(imei_ui.target[a[5]-'0'],16);return 1;}
  if(!strncmp(a,"ia",2)) {
    int action=0;
    const int allowed[]={IA_PREVIEW,IA_DEFAULT,IA_BEGIN,IA_CONTINUE,IA_VERIFY,IA_RESTORE,IA_REBOOT,IA_CANCEL};
    for(size_t i=0;i<sizeof(allowed)/sizeof(*allowed);i++){char id[16];snprintf(id,sizeof(id),"ia%d",allowed[i]);if(!strcmp(a,id))action=allowed[i];}
    if(action&&imei_can(action)) {
      if(action==IA_PREVIEW||action==IA_DEFAULT||action==IA_VERIFY)imei_send(action);
      else {const char *token=action==IA_BEGIN?data.imei.preview:data.imei.op;if(!imei_hex(token)||(action==IA_BEGIN&&!data.imei.expires))return 1;
        imei_ui.action=action;imei_ui.rev=data.imei.rev;snprintf(imei_ui.token,sizeof(imei_ui.token),"%s",token);imei_ui.stage=1;imei_ui.risk=0;}
    }return 1;
  }
  return 0;
}
