/* Compact 320x480 views, using the existing renderer and touch hit pipeline. */
static void render_imei(void) {
  const struct imei_info *s=&data.imei;char tmp[96];
  header("IMEI / NV550");
  if(imei_ui.key>=0) {
    box(12,70,296,24,"head",imei_ui.key?"NV record 1":"NV record 0");
    box(12,102,296,24,"head",imei_ui.input);
    sbox(12,134,296,20,"muted",L("15 ASCII digits · OK edits only","15 位 ASCII 数字 · 确定仅保存输入"));
    for(int i=0;i<9;i++) {char id[12],label[2]={(char)('1'+i),0};snprintf(id,sizeof(id),"ik%d",i+1);linkbox(12+(i%3)*100,166+(i/3)*56,96,52,id,label,1,PLAIN);}
    linkbox(12,334,96,52,"ikclear",L("Clear","清空"),1,PLAIN);
    linkbox(112,334,96,52,"ik0","0",1,PLAIN);
    linkbox(212,334,96,52,"ikbs",L("Delete","退格"),1,PLAIN);
    linkbox(12,402,142,52,"ikcancel",L("Cancel","取消"),1,PLAIN);
    linkbox(166,402,142,52,"ikok","OK",imei_luhn(imei_ui.input)&&imei_can(IA_PREVIEW),PRIMARY);return;
  }
  if(imei_ui.stage) {
    sbox(12,72,296,24,"head",imei_action_label(imei_ui.action));
    box(12,108,296,92,"wrap warn",L("Authorized device only. Two reboots; all networks may disconnect. A verified private backup is required. Never blindly repeat a request.","仅限授权设备。需要两次重启，网络会中断。必须保留经验证的私密备份；勿盲目重发。"));
    sbox(12,204,296,24,"head",s->mask[0][0]?s->mask[0]:L("Keep NV0","保留 NV0"));
    if(!strcmp(s->secondary,"active"))sbox(12,230,296,24,"head",s->mask[1][0]?s->mask[1]:L("Keep NV1","保留 NV1"));
    if(imei_ui.stage==1) {
      switch_row(12,272,296,56,"irisk","irisk",L("I accept the risk","我已知晓风险"),L("Explicit consent required","必须明确勾选"),imei_ui.risk,imei_consent_valid());
      linkbox(12,394,142,56,"iabort",L("Cancel","取消"),1,PLAIN);
      linkbox(166,394,142,56,"inext",L("Next","下一步"),imei_ui.risk&&imei_consent_valid(),DANGER);
    } else {
      const char *text=imei_ui.action==IA_RESTORE?"RESTORE IMEI":imei_ui.action==IA_CANCEL?"CANCEL IMEI":imei_ui.action==IA_REBOOT?L("CONFIRM REBOOT","确认重启"):"CHANGE IMEI";
      box(12,274,296,72,"wrap bad",L("Second confirmation. This queues the selected operation, NOT verified success.","第二次确认：仅排队执行所选操作，不代表已验证成功。"));
      linkbox(12,354,296,52,"isend",text,imei_ui.risk&&imei_consent_valid(),DANGER);
      linkbox(12,414,296,52,"iabort",L("Cancel","取消"),1,PLAIN);
    }
    return;
  }
  snprintf(tmp,sizeof(tmp),"%s · %s",s->has?s->phase:"unavailable",s->has?s->code:"not_available");
  sbox(12,64,296,18,"muted",tmp);
  box(12,84,296,20,"head",s->current[0][0]?s->current[0]:L("NV0 · not checked","NV0 · 未检查"));
  if(!strcmp(s->secondary,"active"))box(12,106,296,20,"head",s->current[1][0]?s->current[1]:L("NV1 · unknown","NV1 · 未知"));
  else sbox(12,106,296,20,"muted",L("NV1 inactive / unknown · keep","NV1 未启用或未知 · 保留"));
  int invalid=!strcmp(s->code,"original_validation_failed")||(s->current[0][0]&&!imei_luhn(s->current[0]))||(s->current[1][0]&&!imei_luhn(s->current[1]));
  sbox(12,128,296,20,invalid?"bad":"muted",invalid?L("Original invalid / UNVERIFIED; blocked","原始值异常 / 未验证，禁止写入"):L("Native NV records, not physical SIM slots","原生 NV 记录，不代表实体 SIM 卡槽"));
  linkbox(12,150,142,48,"icheck",L("CHECK","检查"),imei_can(IA_CHECK),PRIMARY);
  linkbox(166,150,142,48,"irefresh",L("Refresh status","刷新状态"),!imei_ui.sent,PLAIN);
  const int edit=imei_can(IA_PREVIEW)&&!invalid,preview=!strcmp(s->phase,"preview");
  linkbox(12,204,222,48,"iedit0",preview&&s->mask[0][0]?s->mask[0]:imei_ui.target[0][0]?imei_ui.target[0]:L("NV0 · Keep / input","NV0 · 保留 / 输入"),edit,PLAIN);
  linkbox(240,204,68,48,"ikeep0",L("Keep","保留"),edit,PLAIN);
  if(!strcmp(s->secondary,"active")) {
    linkbox(12,258,222,48,"iedit1",preview&&s->mask[1][0]?s->mask[1]:imei_ui.target[1][0]?imei_ui.target[1]:L("NV1 · Keep / input","NV1 · 保留 / 输入"),edit,PLAIN);
    linkbox(240,258,68,48,"ikeep1",L("Keep","保留"),edit,PLAIN);
  }
  const int actions[]={IA_PREVIEW,IA_DEFAULT,IA_BEGIN,IA_CONTINUE,IA_VERIFY,IA_REBOOT,IA_RESTORE,IA_CANCEL};int count=0,shown=0;
  for(size_t i=0;i<sizeof(actions)/sizeof(*actions);i++)if(s->allowed&actions[i])count++;
  int start=count>4?(imei_ui.action_page%((count+2)/3))*3:0,limit=count>4?3:4;
  for(size_t i=0;i<sizeof(actions)/sizeof(*actions);i++)if(s->allowed&actions[i]) {
    int n=shown++;if(n<start||n>=start+limit)continue;
    int slot=n-start,a=actions[i];char id[20];snprintf(id,sizeof(id),"ia%d",a);
    linkbox(12+(slot%2)*154,312+(slot/2)*54,142,48,id,imei_action_label(a),imei_can(a)&&(a!=IA_PREVIEW||imei_targets_valid()),a==IA_PREVIEW||a==IA_DEFAULT||a==IA_VERIFY?PRIMARY:DANGER);
  }
  if(count>4)linkbox(166,366,142,48,"imore",L("More actions","更多操作"),1,PLAIN);
  const char *hint=imei_ui.error[0]?imei_ui.error:!imei_fresh()?L("Unavailable / stale. Refresh status.","不可用 / 状态过期，请刷新状态。"):
    imei_ui.sent||s->busy?L("Queued / busy. Status is not success.","已排队 / 忙碌。状态不代表成功。"):
    !strcmp(s->phase,"reboot_to_enable")||!strcmp(s->phase,"reboot_to_verify")?L("Manual reboot allowed; then refresh and continue / verify.","可手动重启，再刷新并继续 / 验证。"):
    L("Two reboots + network loss. Keep backup.","两次重启、网络中断；请保留备份。");
  box(12,432,296,44,"wrap warn",hint);
}
