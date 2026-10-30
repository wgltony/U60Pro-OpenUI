/* Local IMEI input and consent. No timer, render or keyboard completion submits. */
enum { IA_CHECK=1,IA_PREVIEW=2,IA_BEGIN=4,IA_CONTINUE=8,IA_VERIFY=16,IA_RESTORE=32,IA_REBOOT=64,IA_CANCEL=128 };
static struct {
  char target[2][16],input[16],error[128],token[33];
  unsigned long long rev, sent, seen;
  int key,stage,action,risk,uncertain,action_page;
} imei_ui={.key=-1};
static void imei_clear(void) { relay_wipe(&imei_ui,sizeof(imei_ui));imei_ui.key=-1; }
/* Local monotonic arrival/latency, independent of router or host epoch. Retain
 * the preview deadline across polls and failed reads; a poll cannot renew it. */
static struct {
  int valid;
  long long started,received,preview_until;
  unsigned long long rev,preview_rev;
  char preview[33];
} imei_clock;
static void imei_observe(const struct imei_info *s,long long started) {
  if(!s->has){imei_clock.valid=0;return;}
  long long until=started+(long long)s->expires*1000;
  if(imei_hex(s->preview)&&imei_clock.preview_rev==s->rev&&!strcmp(imei_clock.preview,s->preview)&&imei_clock.preview_until<until)until=imei_clock.preview_until;
  imei_clock.valid=1;imei_clock.started=started;imei_clock.received=monotonic_ms();imei_clock.rev=s->rev;
  if(imei_hex(s->preview)){snprintf(imei_clock.preview,sizeof(imei_clock.preview),"%s",s->preview);imei_clock.preview_rev=s->rev;imei_clock.preview_until=until;}
}
static int imei_fresh(void) {
  long long now=monotonic_ms();
  return data.imei.has&&imei_clock.valid&&data.imei.rev==imei_clock.rev&&imei_clock.received>=imei_clock.started&&
    imei_clock.received-imei_clock.started<=10000&&now>=imei_clock.received&&now-imei_clock.started<=10000;
}
static int imei_normal_code(const char *s) {
  static const char *const codes[]={"not_checked","ready","preview_ready","working","awaiting_first_reboot","awaiting_write_confirmation","awaiting_final_reboot","verified","restored","cancelled","recovery_required"};
  for(size_t i=0;i<sizeof(codes)/sizeof(*codes);i++)if(!strcmp(s,codes[i]))return 1;
  return 0;
}
static int imei_can(int bit) {
  const struct imei_info *s=&data.imei;
  return imei_fresh()&&!pending&&!data.busy&&!s->busy&&!imei_ui.sent&&!imei_ui.uncertain&&(s->allowed&bit)&&
    strcmp(s->phase,"unsupported")&&
    (bit!=IA_BEGIN||(imei_hex(s->preview)&&s->expires>0&&imei_clock.preview_rev==s->rev&&!strcmp(imei_clock.preview,s->preview)&&monotonic_ms()<imei_clock.preview_until))&&
    ((bit!=IA_CONTINUE&&bit!=IA_VERIFY&&bit!=IA_RESTORE&&bit!=IA_REBOOT&&bit!=IA_CANCEL)||imei_hex(s->op))&&
    ((bit!=IA_PREVIEW&&bit!=IA_BEGIN&&bit!=IA_CONTINUE)||
      ((!s->current[0][0]||imei_luhn(s->current[0]))&&(!s->current[1][0]||imei_luhn(s->current[1]))))&&
    (bit==IA_CHECK||bit==IA_VERIFY||imei_normal_code(s->code));
}
static int imei_targets_valid(void) {
  const struct imei_info *s=&data.imei;const char *a=imei_ui.target[0],*b=imei_ui.target[1];
  if(!a[0]&&!b[0])return 0;
  if((a[0]&&!imei_luhn(a))||(b[0]&&!imei_luhn(b)))return 0;
  if(strcmp(s->secondary,"active")&&b[0])return 0;
  if((!a[0]||!strcmp(a,s->current[0]))&&(!b[0]||!strcmp(b,s->current[1])))return 0;
  /* Cross-original targets can leave an interrupted hybrid with duplicate records. */
  if((a[0]&&!strcmp(a,s->current[1]))||(b[0]&&!strcmp(b,s->current[0])))return 0;
  return strcmp(s->secondary,"active")||strcmp(a[0]?a:s->current[0],b[0]?b:s->current[1]);
}
static int imei_consent_valid(void) {
  const struct imei_info *s=&data.imei;
  if(!imei_can(imei_ui.action)||s->rev!=imei_ui.rev)return 0;
  return imei_ui.action==IA_BEGIN?(s->expires>0&&imei_hex(s->preview)&&!strcmp(s->preview,imei_ui.token)):
    imei_hex(s->op)&&!strcmp(s->op,imei_ui.token);
}
static const char *imei_action_name(int a) {
  return a==IA_CHECK?"check":a==IA_PREVIEW?"preview":a==IA_BEGIN?"begin":a==IA_CONTINUE?"continue":
    a==IA_VERIFY?"verify":a==IA_RESTORE?"restore":a==IA_REBOOT?"reboot":"cancel";
}
static const char *imei_action_label(int a) {
  return a==IA_CHECK?L("CHECK","检查"):a==IA_PREVIEW?L("Preview","预览"):a==IA_BEGIN?L("Begin","开始变更"):
    a==IA_CONTINUE?L("Continue","继续写入"):a==IA_VERIFY?L("Verify","最终验证"):a==IA_RESTORE?L("Restore backup","恢复备份"):
    a==IA_REBOOT?L("Reboot","重启"):L("Cancel operation","取消操作");
}
static void imei_track(void) {
  if(page!=IMEI_PAGE){imei_clear();return;}
  if(data.imei.has&&data.imei.rev<imei_ui.seen)imei_ui.uncertain=1;
  if(data.imei.has&&data.imei.rev>imei_ui.seen)imei_ui.seen=data.imei.rev;
  if(!strcmp(data.imei.phase,"complete")||!strcmp(data.imei.code,"verified")||!strcmp(data.imei.code,"restored")||!strcmp(data.imei.code,"cancelled")) {
    relay_wipe(imei_ui.target,sizeof(imei_ui.target));relay_wipe(imei_ui.input,sizeof(imei_ui.input));imei_ui.key=-1;
  }
  if(imei_ui.sent&&data.imei.has&&data.imei.rev>imei_ui.sent)imei_ui.sent=0;
  if(imei_ui.stage&&!imei_consent_valid()) {imei_ui.stage=0;imei_ui.risk=0;snprintf(imei_ui.error,sizeof(imei_ui.error),"%s",L("Status changed. Refresh; confirm again.","状态已变化，请刷新后重新确认。"));}
  if(imei_ui.key>=0&&(!imei_fresh()||data.imei.rev!=imei_ui.rev)) {relay_wipe(imei_ui.input,sizeof(imei_ui.input));imei_ui.key=-1;}
  if(strcmp(data.imei.secondary,"active"))relay_wipe(imei_ui.target[1],sizeof(imei_ui.target[1]));
}
