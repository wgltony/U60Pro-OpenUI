/* Relay editor: local-only state; no credential appears in status or action IDs.
 * Passwords are never read back from the agent. GPL-3.0-or-later. */
enum { RV_STATUS, RV_SAVED, RV_SCAN, RV_DETAIL, RV_EDIT, RV_KEYBOARD };
enum { RK_SSID, RK_PASSWORD };
struct relay_editor {
  char ssid[33], password[65], before[65];
  unsigned long long rev, gen, id;
  int active, security, hidden, saved, keep, key, keymode, show, overflow, from_scan, scan_index, original_security, has_secret;
};
static struct relay_editor relay_edit;
static int relay_view, relay_offset;
/* Non-secret rendering versions: never retain a second copy of the input. */
static unsigned long long relay_key_revision, relay_key_epoch;
/* Volatile writes cannot be optimized away. Also clear abandoned bytes after backspace. */
static void relay_wipe(void *p, size_t n) {
  volatile unsigned char *v = p;
  while (n--) *v++ = 0;
}
static void relay_editor_clear(void) { relay_wipe(&relay_edit,sizeof(relay_edit));relay_wipe(confirm_req,sizeof(confirm_req));relay_key_revision++;relay_key_epoch++; }
static char *relay_input(void) { return relay_edit.key==RK_SSID?relay_edit.ssid:relay_edit.password; }
static size_t relay_input_limit(void) { return relay_edit.key==RK_SSID?32:64; }
static void relay_key_open(int field) {
  relay_key_revision++;relay_key_epoch++;
  relay_edit.key=field;relay_edit.keymode=0;relay_edit.show=0;relay_edit.overflow=0;
  relay_wipe(relay_edit.before,sizeof(relay_edit.before));
  if(field==RK_SSID)snprintf(relay_edit.before,sizeof(relay_edit.before),"%s",relay_input());
  relay_view=RV_KEYBOARD;
}
/* Only printable ASCII can be entered. UTF-8 SSIDs selected from a list are preserved
 * byte-for-byte and cannot accidentally be changed by this ASCII-only editor. */
static int relay_ascii(const char *s) {
  for(;*s;s++)if((unsigned char)*s<32||(unsigned char)*s>126)return 0;
  return 1;
}
static void relay_key_char(unsigned c) {
  char *s=relay_input();size_t n=strlen(s);
  if(c<32||c>126)return;
  relay_key_revision++;
  if(n>=relay_input_limit()){relay_edit.overflow=1;return;}
  s[n]=(char)c;s[n+1]=0;relay_edit.overflow=0;
  if(relay_edit.key==RK_PASSWORD)relay_edit.keep=0;
}
static void relay_key_backspace(void) {
  relay_key_revision++;
  char *s=relay_input();size_t n=strlen(s);
  if(n)s[n-1]=0;
  relay_edit.overflow=0;
  if(relay_edit.key==RK_PASSWORD)relay_edit.keep=0;
}
static const char *relay_key_row(int row) {
  static const char *const keys[4][3]={
    {"qwertyuiop","asdfghjkl","zxcvbnm"},
    {"QWERTYUIOP","ASDFGHJKL","ZXCVBNM"},
    {"1234567890","!\"#$%&'()*","+,-./:;<=>"},
    {"?@[\\]^_`{|","}~","1234567890"}
  };
  return keys[relay_edit.keymode][row];
}
static int relay_password_valid(void) {
  size_t n=strlen(relay_edit.password);
  if(!relay_edit.security)return !n;
  if(relay_edit.keep&&relay_edit.saved&&relay_edit.has_secret&&relay_edit.security==relay_edit.original_security)return 1;
  if(n>=8&&n<=63)return 1;
  if(n!=64||(relay_edit.security!=1&&relay_edit.security!=3))return 0;
  for(size_t i=0;i<n;i++)if(!strchr("0123456789abcdefABCDEF",relay_edit.password[i]))return 0;
  return 1;
}
static int relay_keyboard_tap(const char *a) {
  if(relay_view!=RV_KEYBOARD)return 0;
  if(!strncmp(a,"rk",2)&&strlen(a)==4) {
    unsigned c=0;
    for(int i=2;i<4;i++) {
      const char *p=strchr("0123456789abcdef",a[i]);if(!p)return 1;
      c=c*16+(unsigned)(p-"0123456789abcdef");
    }
    relay_key_char(c);
  } else if(!strcmp(a,"rshift"))relay_edit.keymode=relay_edit.keymode==1?0:1;
  else if(!strcmp(a,"rsymbol"))relay_edit.keymode=relay_edit.keymode==2?3:2;
  else if(!strcmp(a,"rletters"))relay_edit.keymode=0;
  else if(!strcmp(a,"rspace"))relay_key_char(' ');
  else if(!strcmp(a,"rbs"))relay_key_backspace();
  else if(!strcmp(a,"rclear")) {
    relay_key_revision++;
    relay_wipe(relay_input(),relay_input_limit()+1);relay_edit.overflow=0;
    if(relay_edit.key==RK_PASSWORD)relay_edit.keep=0;
  } else if(!strcmp(a,"rshow"))relay_edit.show=!relay_edit.show;
  else if(!strcmp(a,"rkdone")) {
    relay_wipe(relay_edit.before,sizeof(relay_edit.before));relay_edit.show=0;relay_view=RV_EDIT;
  } else if(!strcmp(a,"rkcancel")||!strcmp(a,"back")) {
    /* Cancel never restores a password into memory: keeping an existing saved secret
     * is explicit on the form. Cancelling an SSID field restores its prior bytes. */
    relay_wipe(relay_input(),relay_input_limit()+1);
    if(relay_edit.key==RK_SSID)snprintf(relay_edit.ssid,sizeof(relay_edit.ssid),"%s",relay_edit.before);
    else relay_edit.keep=relay_edit.saved&&relay_edit.has_secret&&relay_edit.security==relay_edit.original_security;
    relay_wipe(relay_edit.before,sizeof(relay_edit.before));relay_edit.show=0;relay_view=RV_EDIT;
  }
  return 1;
}
