/* Proxy and Clash records from the agent, and text the panel can display.
 * Groups and nodes come with names but are only ever sent back as indices plus the
 * list generation, so no name has to be parsed by the agent.
 * Copyright (c) 2026 ZweiChen. GPL-3.0-or-later. */
#include <limits.h>
enum { BK_OFF, BK_WG, BK_CLASH, BK_TS };
/* A diagnosis figure the agent did not report. */
#define NO_FIGURE LLONG_MIN
struct backend_info {
  int has, active, conflict;
  unsigned long long rev[3]; /* WireGuard, Clash */
  int ready[3], error[3];
  char reason[3][12];
};
struct clash_info {
  int has, enabled, running, connected, installed;
  unsigned long long rev, uptime, memory_kib, active;
  char mode[8], version[33], error[97];
};
struct sub_info {
  unsigned long long id, updated, up, down, total, expire;
  int active, downloaded, proxies, failed;
  char name[193];
};
struct core_info {
  int has, available, newer;
  unsigned long long bytes, total, free;
  char version[33], source[13], previous[33], latest[33], published[11];
  char kind[13], state[13], step[17], error[97];
};
struct check_info { char id[13], status[9]; long long v[4]; };
struct diag_info {
  int has, running, n;
  unsigned long long finished;
  char verdict[17];
  struct check_info checks[8];
};
/* eSIM: the card in the tray and its profiles; no EID, only an ICCID's last digits. */
struct esim_row { int index, enabled; char cls[16], name[97], tail[5]; };
struct esim_info {
  int has, installed, reboot, total, n, firmware_ready; /* firmware_ready -1: unknown */
  unsigned long long gen;
  char card[9], kind[16], state[12], step[40], error[97];
  struct esim_row rows[8];
};
/* The Wi-Fi uplink: no password, BSSID or gateway. state is one letter: o off,
 * u unavailable, c connecting, d getting an address, k checking, n online, i no internet,
 * p needs a web login, g login mode (traffic on Wi-Fi until signed in). */
struct uplink_info {
  int has, enabled, saved, has_signal, signal;
  unsigned long long rev, rx, tx, since;
  char state, reason[12], ssid[65], band[4], ip[16];
};
/* L/l/w are credential-free relay catalogues, tied to R revision. */
struct relay_saved { unsigned long long id; int preferred,security,hidden,has_password; char ssid[33]; };
struct relay_scan { int index,security,signal,frequency; char ssid[33]; };
struct relay_catalog {
  int has, state, saved_count,scan_count,ns,nw;
  unsigned long long rev,gen,stamp;
  struct relay_saved saved[8];struct relay_scan scan[60];
};
/* k: Cloudflare IP optimization. problem: - nonode offline clash failed. */
struct cfopt_info {
  int has, enabled, running, interval;
  unsigned long long rev, last_run;
  unsigned tls_ms, speed_kbps;
  char phase[13], ip[16], colo[9], problem[8];
};
struct tailscale_row { int index, selected; char name[49]; };
struct tailscale_info {
  int has, enabled, error, n;
  int has_ssh, remote_ssh, ssh_service; /* h: 0 off, 1 on, 2 unknown */
  unsigned long long rev, gen, stamp;
  char state[20], selected[49];
  struct tailscale_row rows[32];
};
/* A version 1: privileged access, never credentials. Absent/invalid is unknown. */
struct admin_info {
  int has, busy, adb_running, adb_exposed, adb_boot, ssh_running, ssh_boot;
  int adb_available, ssh_available;
  unsigned long long rev, stamp;
};
struct toolbox_info {
  int has;
  long long rssi, rsrp, rsrq, sinr;
  char radio[16], band[16], net_select[16], lte_bands[32], nr_bands[32], pci[16];
  unsigned long long kbps_down, kbps_up, cpu_pct, mem_pct;
  long long temp_mc;
  unsigned long long bloat_cnt;
  int wifi_2g_on, wifi_5g_on;
  unsigned long long wifi_clients;
  char wifi_ssid[49], usb_mode[16];
  int usb_adb;
  unsigned long long ttl_val;
  int chg_enabled, chg_limit_on;
  unsigned long long chg_max, chg_stop, chg_start, batt_mv;
  long long batt_ma;
  char batt_health[24];
  /* Appended fields; an older agent sends 32 and has_usb_lan stays 0 (unknown). */
  /* usb_lan_state: 0 not shared, 1 bridged and waiting for a USB host, 2 host linked. */
  int has_usb_lan, usb_lan_on, usb_lan_state;
  char usb_lan_iface[16];
  /* Further appended: transmit power percent and dBm per band (0 = unknown). */
  int has_wifi_power;
  unsigned long long wifi_pct_2g, wifi_pct_5g, wifi_dbm_2g, wifi_dbm_5g;
};
struct group_row { int index, selectable, members, now; char type[16], name[193], now_name[193]; };
/* state: -1 no answer, 0 loading, 1 ready, 2 Clash not running. */
struct groups_view { int state, total, offset, n; unsigned long long gen; struct group_row rows[4]; };
struct node_row { int index; long long delay; char type[16], name[193]; };
struct nodes_view {
  int ok, group, total, offset, n, selectable, now;
  unsigned long long gen;
  char type[16], name[193];
  struct node_row rows[4];
};
/* Unifont covers the BMP only. Flags (regional-indicator pairs) become their ASCII
 * letters, e.g. U+1F1ED U+1F1F0 -> "HK"; other characters outside the BMP, variation
 * selectors, joiners and broken UTF-8 are dropped, then spaces are tidied. In place. */
static void display_text(char *s) {
  const unsigned char *in = (const unsigned char *)s;
  char *out = s;
  while (*in) {
    unsigned cp;
    int len = in[0] < 0x80 ? 1 : (in[0] & 0xe0) == 0xc0 ? 2 : (in[0] & 0xf0) == 0xe0 ? 3 : (in[0] & 0xf8) == 0xf0 ? 4 : 0;
    if (!len) { in++; continue; }
    cp = len == 1 ? in[0] : in[0] & (0x7f >> len);
    int i = 1;
    for (; i < len && (in[i] & 0xc0) == 0x80; i++) cp = cp << 6 | (in[i] & 0x3f);
    if (i < len) { in++; continue; }
    if (cp >= 0x1f1e6 && cp <= 0x1f1ff) *out++ = (char)('A' + (cp - 0x1f1e6));
    else if (!(cp > 0xffff || (cp >= 0xfe00 && cp <= 0xfe0f) || cp == 0x200d || cp == 0x20e3)) {
      memmove(out, in, (size_t)len);
      out += len;
    }
    in += len;
  }
  *out = 0;
  char *r = s, *w = s;
  while (*r == ' ') r++;
  for (; *r; r++) if (*r != ' ' || w[-1] != ' ') *w++ = *r;
  while (w > s && w[-1] == ' ') w--;
  *w = 0;
}
/* Splits a record on tabs in place; the last field keeps any further tabs. */
static int split_tabs(char *line, char **f, int max) {
  int n = 0;
  while (n < max) {
    f[n++] = line;
    if (!(line = strchr(line, '\t'))) break;
    *line++ = 0;
  }
  return n;
}
/* The agent sends "-" for an empty field. Text longer than the buffer (the agent sizes
 * its fields to fit) ends before the first character that does not fit whole. */
static void copy_field(char *dst, size_t cap, const char *src) {
  if (!strcmp(src, "-")) src = "";
  size_t n = strlen(src);
  if (n >= cap)
    for (n = cap - 1; n > 0 && ((unsigned char)src[n] & 0xc0) == 0x80; n--) {}
  memcpy(dst, src, n);
  dst[n] = 0;
}
static void copy_name(char *dst, size_t cap, const char *src) { copy_field(dst, cap, src); display_text(dst); }
static unsigned long long u64_field(const char *s) { return strtoull(s, NULL, 10); }
static long long figure_field(const char *s) { return strcmp(s, "-") ? strtoll(s, NULL, 10) : NO_FIGURE; }
#define COPY(dst, src) copy_field(dst, sizeof(dst), src)
/* B active conflict, then per backend: revision ready reason error. */
static void parse_backend(char **f, int n, struct backend_info *b) {
  if (n < 11) return;
  b->active = !strcmp(f[1], "wireguard") ? BK_WG : !strcmp(f[1], "clash") ? BK_CLASH : !strcmp(f[1], "tailscale") ? BK_TS : BK_OFF;
  b->conflict = atoi(f[2]) != 0;
  for (int i = 0; i < (n >= 15 ? 3 : 2); i++) {
    b->rev[i] = u64_field(f[3 + 4 * i]);
    b->ready[i] = atoi(f[4 + 4 * i]) != 0;
    COPY(b->reason[i], f[5 + 4 * i]);
    b->error[i] = atoi(f[6 + 4 * i]) != 0;
  }
  b->has = 1;
}
/* K revision enabled running connected mode uptime memory active installed version error */
static void parse_clash(char **f, int n, struct clash_info *c) {
  if (n < 12) return;
  c->rev = u64_field(f[1]);
  c->enabled = atoi(f[2]) != 0;
  c->running = atoi(f[3]) != 0;
  c->connected = atoi(f[4]) != 0;
  COPY(c->mode, f[5]);
  c->uptime = u64_field(f[6]);
  c->memory_kib = u64_field(f[7]);
  c->active = u64_field(f[8]);
  c->installed = atoi(f[9]) != 0;
  COPY(c->version, f[10]);
  COPY(c->error, f[11]);
  c->has = 1;
}
/* U id active downloaded updated upload download total expire proxies failed name */
static int parse_sub(char **f, int n, struct sub_info *s) {
  if (n < 12) return 0;
  s->id = u64_field(f[1]);
  if (!s->id) return 0;
  s->active = atoi(f[2]) != 0;
  s->downloaded = atoi(f[3]) != 0;
  s->updated = u64_field(f[4]);
  s->up = u64_field(f[5]);
  s->down = u64_field(f[6]);
  s->total = u64_field(f[7]);
  s->expire = u64_field(f[8]);
  s->proxies = atoi(f[9]);
  s->failed = atoi(f[10]) != 0;
  copy_name(s->name, sizeof(s->name), f[11]);
  return 1;
}
/* M available version source previous latest published newer kind state step bytes
 * total free error; "M 0" from an agent without core updates. */
static void parse_core(char **f, int n, struct core_info *c) {
  if (n < 2) return;
  c->has = 1;
  c->available = atoi(f[1]) != 0 && n >= 15;
  if (!c->available) return;
  COPY(c->version, f[2]);
  COPY(c->source, f[3]);
  COPY(c->previous, f[4]);
  COPY(c->latest, f[5]);
  COPY(c->published, f[6]);
  c->newer = atoi(f[7]) != 0;
  COPY(c->kind, f[8]);
  COPY(c->state, f[9]);
  COPY(c->step, f[10]);
  c->bytes = u64_field(f[11]);
  c->total = u64_field(f[12]);
  c->free = u64_field(f[13]);
  COPY(c->error, f[14]);
}
/* X running finished verdict, then x id status figure*4 */
static void parse_diag(char **f, int n, struct diag_info *d) {
  if (n < 4) return;
  d->has = 1;
  d->running = atoi(f[1]) != 0;
  d->finished = u64_field(f[2]);
  COPY(d->verdict, f[3]);
}
static void parse_check(char **f, int n, struct diag_info *d) {
  if (n < 7 || d->n >= 8) return;
  struct check_info *c = &d->checks[d->n++];
  COPY(c->id, f[1]);
  COPY(c->status, f[2]);
  for (int i = 0; i < 4; i++) c->v[i] = figure_field(f[3 + i]);
}
/* Q installed card generation count reboot kind state step firmware_ready error */
static void parse_esim(char **f, int n, struct esim_info *e) {
  if (n < 11) return;
  e->has = 1;
  e->installed = atoi(f[1]) != 0;
  COPY(e->card, f[2]);
  e->gen = u64_field(f[3]);
  e->total = atoi(f[4]);
  e->reboot = atoi(f[5]) != 0;
  COPY(e->kind, f[6]);
  COPY(e->state, f[7]);
  COPY(e->step, f[8]);
  e->firmware_ready = !strcmp(f[9], "-") ? -1 : atoi(f[9]) != 0;
  COPY(e->error, f[10]);
}
/* q index enabled class name iccid-tail */
static void parse_esim_row(char **f, int n, struct esim_info *e) {
  if (n < 6 || e->n >= 8) return;
  struct esim_row *r = &e->rows[e->n++];
  r->index = atoi(f[1]);
  r->enabled = atoi(f[2]) != 0;
  COPY(r->cls, f[3]);
  copy_name(r->name, sizeof(r->name), f[4]);
  COPY(r->tail, f[5]);
}
/* R revision enabled state reason ssid signal band ip rx tx online_since saved */
static void parse_uplink(char **f, int n, struct uplink_info *u) {
  if (n < 13 || !f[3][0] || f[3][1] || !strchr("oucdknipg", f[3][0])) return;
  u->rev = u64_field(f[1]);
  u->enabled = atoi(f[2]) != 0;
  u->state = f[3][0];
  COPY(u->reason, f[4]);
  copy_name(u->ssid, sizeof(u->ssid), f[5]);
  u->has_signal = strcmp(f[6], "-") != 0;
  u->signal = u->has_signal ? atoi(f[6]) : 0;
  COPY(u->band, f[7]);
  COPY(u->ip, f[8]);
  u->rx = u64_field(f[9]);
  u->tx = u64_field(f[10]);
  u->since = u64_field(f[11]);
  u->saved = atoi(f[12]);
  u->has = 1;
}
/* G step down up: recent speed for the Signal page's chart, kbit/s per `step` seconds as
 * comma lists, oldest first. n stays 0 with an older agent. */
#define CHART_POINTS 60
struct speed_chart { int n, step; unsigned down[CHART_POINTS], up[CHART_POINTS]; };
static int csv_u32(const char *s, unsigned *out, int max) {
  int n=0;
  while(*s&&n<max) {
    char *end;unsigned long v=strtoul(s,&end,10);
    if(end==s||(*end&&*end!=','))return -1;
    out[n++]=v>4000000000UL?4000000000U:(unsigned)v;
    s=*end?end+1:end;
  }
  return *s?-1:n;
}
static void parse_speed_chart(char **f, int n, struct speed_chart *c) {
  if(n<4)return;
  int step=atoi(f[1]),nd=csv_u32(f[2],c->down,CHART_POINTS),nu=csv_u32(f[3],c->up,CHART_POINTS);
  c->n=step>0&&step<=60&&nd>0&&nd==nu?nd:0;c->step=c->n?step:0;
}
/* I mac band signal secs ip name: a Wi-Fi station. i mac name: a blocked MAC. */
struct wifi_sta { char mac[18], ip[16], name[49]; int band; long long signal; unsigned long long secs; };
struct wifi_blocked { char mac[18], name[49]; };
static int valid_mac(const char *m) {
  if(strlen(m)!=17)return 0;
  for(int i=0;i<17;i++)if(i%3==2?m[i]!=':':!m[i]||!strchr("0123456789abcdefABCDEF",m[i]))return 0;
  return 1;
}
static int parse_wifi_sta(char **f, int n, struct wifi_sta *w) {
  if(n<7||!valid_mac(f[1]))return 0;
  COPY(w->mac,f[1]);w->band=atoi(f[2])==5?5:2;w->signal=strtoll(f[3],NULL,10);w->secs=u64_field(f[4]);
  COPY(w->ip,f[5]);copy_name(w->name,sizeof(w->name),f[6]);
  return 1;
}
static int parse_wifi_blocked(char **f, int n, struct wifi_blocked *b) {
  if(n<3||!valid_mac(f[1]))return 0;
  COPY(b->mac,f[1]);copy_name(b->name,sizeof(b->name),f[2]);
  return 1;
}
/* H rssi rsrp rsrq sinr radio band net_select lte_bands nr_bands pci kbps_down kbps_up
 * cpu_pct mem_pct temp_mc bloat_cnt wifi_2g_on wifi_5g_on wifi_clients wifi_ssid
 * usb_mode usb_adb ttl_val chg_enabled chg_limit_on chg_max chg_stop chg_start
 * batt_mv batt_ma batt_health [usb_lan_on usb_lan_state usb_lan_iface]
 * [wifi_pct_2g wifi_pct_5g wifi_dbm_2g wifi_dbm_5g] */
static void parse_toolbox(char **f, int n, struct toolbox_info *h) {
  if (n < 32) return;
  h->has = 1;
  h->rssi = strtoll(f[1], NULL, 10);
  h->rsrp = strtoll(f[2], NULL, 10);
  h->rsrq = strtoll(f[3], NULL, 10);
  h->sinr = strtoll(f[4], NULL, 10);
  COPY(h->radio, f[5]);
  COPY(h->band, f[6]);
  COPY(h->net_select, f[7]);
  COPY(h->lte_bands, f[8]);
  COPY(h->nr_bands, f[9]);
  COPY(h->pci, f[10]);
  h->kbps_down = u64_field(f[11]);
  h->kbps_up = u64_field(f[12]);
  h->cpu_pct = u64_field(f[13]);
  h->mem_pct = u64_field(f[14]);
  h->temp_mc = strtoll(f[15], NULL, 10);
  h->bloat_cnt = u64_field(f[16]);
  h->wifi_2g_on = atoi(f[17]) != 0;
  h->wifi_5g_on = atoi(f[18]) != 0;
  h->wifi_clients = u64_field(f[19]);
  copy_name(h->wifi_ssid, sizeof(h->wifi_ssid), f[20]);
  COPY(h->usb_mode, f[21]);
  h->usb_adb = atoi(f[22]) != 0;
  h->ttl_val = u64_field(f[23]);
  h->chg_enabled = atoi(f[24]) != 0;
  h->chg_limit_on = atoi(f[25]) != 0;
  h->chg_max = u64_field(f[26]);
  h->chg_stop = u64_field(f[27]);
  h->chg_start = u64_field(f[28]);
  h->batt_mv = u64_field(f[29]);
  h->batt_ma = strtoll(f[30], NULL, 10);
  COPY(h->batt_health, f[31]);
  h->has_usb_lan = n >= 35;
  if (h->has_usb_lan) {
    h->usb_lan_on = atoi(f[32]) != 0;
    h->usb_lan_state = atoi(f[33]);
    COPY(h->usb_lan_iface, f[34]);
  }
  h->has_wifi_power = n >= 39;
  if (h->has_wifi_power) {
    h->wifi_pct_2g = u64_field(f[35]);
    h->wifi_pct_5g = u64_field(f[36]);
    h->wifi_dbm_2g = u64_field(f[37]);
    h->wifi_dbm_5g = u64_field(f[38]);
  }
}
/* Reply to "groups <offset>": G state generation total offset, g rows. */
static int parse_groups(char *buf, struct groups_view *out) {
  struct groups_view v = {.state = -1};
  char *save, *line = strtok_r(buf, "\n", &save);
  for (; line; line = strtok_r(NULL, "\n", &save)) {
    char *f[8];
    int n = split_tabs(line, f, 8);
    if (!strcmp(f[0], "G") && n >= 5) {
      v.state = atoi(f[1]);
      v.gen = u64_field(f[2]);
      v.total = atoi(f[3]);
      v.offset = atoi(f[4]);
    } else if (!strcmp(f[0], "g") && n >= 8 && v.n < 4) {
      struct group_row *r = &v.rows[v.n++];
      r->index = atoi(f[1]);
      r->selectable = atoi(f[2]) != 0;
      r->members = atoi(f[3]);
      r->now = atoi(f[4]);
      COPY(r->type, f[5]);
      copy_name(r->name, sizeof(r->name), f[6]);
      copy_name(r->now_name, sizeof(r->now_name), f[7]);
    }
  }
  if (v.state < 0) return 0;
  *out = v;
  return 1;
}
/* Reply to "nodes <gen> <group> <offset>": N gen group total offset selectable now type
 * name, n index delay type name (delay -1 untested, 0 failed). */
static int parse_nodes(char *buf, struct nodes_view *out) {
  struct nodes_view v = {0};
  char *save, *line = strtok_r(buf, "\n", &save);
  for (; line; line = strtok_r(NULL, "\n", &save)) {
    char *f[9];
    int n = split_tabs(line, f, 9);
    if (!strcmp(f[0], "N") && n >= 9) {
      v.ok = 1;
      v.gen = u64_field(f[1]);
      v.group = atoi(f[2]);
      v.total = atoi(f[3]);
      v.offset = atoi(f[4]);
      v.selectable = atoi(f[5]) != 0;
      v.now = atoi(f[6]);
      COPY(v.type, f[7]);
      copy_name(v.name, sizeof(v.name), f[8]);
    } else if (!strcmp(f[0], "n") && n >= 5 && v.n < 4) {
      struct node_row *r = &v.rows[v.n++];
      r->index = atoi(f[1]);
      r->delay = strtoll(f[2], NULL, 10);
      COPY(r->type, f[3]);
      copy_name(r->name, sizeof(r->name), f[4]);
    }
  }
  if (!v.ok) return 0;
  *out = v;
  return 1;
}

static int relay_security(const char *s) {
  const char *const names[]={"open","wpa2","wpa3","wpa2_wpa3","unsupported"};
  for(int i=0;i<5;i++)if(!strcmp(s,names[i]))return i;
  return -1;
}
static int relay_number(const char *s,unsigned long long *v) {
  if(!*s)return 0;
  for(const char *p=s;*p;p++)if(*p<'0'||*p>'9')return 0;
  char *end;errno=0;*v=strtoull(s,&end,10);return !errno&&!*end;
}
static int relay_bool(const char *s) { return !strcmp(s,"0")||!strcmp(s,"1"); }
/* Exact A12 schema. Validate everything before publishing, including overflow and
 * boolean spelling: neither missing nor malformed fields may masquerade as OFF. */
static void parse_admin(char **f, int n, struct admin_info *a) {
  struct admin_info next = {0};
  memset(a, 0, sizeof(*a));
  if(n!=12 || strcmp(f[0],"A") || strcmp(f[1],"1") ||
     !relay_number(f[2],&next.rev) ||
     !relay_number(f[3],&next.stamp) || !next.stamp) return;
  for(int i=4;i<12;i++) if(!relay_bool(f[i])) return;
  next.has=1;next.busy=f[4][0]-'0';next.adb_running=f[5][0]-'0';
  next.adb_exposed=f[6][0]-'0';next.adb_boot=f[7][0]-'0';
  next.ssh_running=f[8][0]-'0';next.ssh_boot=f[9][0]-'0';
  next.adb_available=f[10][0]-'0';next.ssh_available=f[11][0]-'0';
  *a=next;
}
/* SSID bytes must stay exact: validate, never sanitize or trim them. */
static int relay_ssid_utf8(const unsigned char *s,size_t n) {
  if(!n||n>32)return 0;
  for(size_t i=0;i<n;) {
    unsigned c=s[i++],cp,min=0;size_t more;
    if(c<0x80){cp=c;more=0;}
    else if(c>=0xc2&&c<=0xdf){cp=c&0x1f;more=1;min=0x80;}
    else if(c>=0xe0&&c<=0xef){cp=c&0x0f;more=2;min=0x800;}
    else if(c>=0xf0&&c<=0xf4){cp=c&7;more=3;min=0x10000;}
    else return 0;
    if(i+more>n)return 0;
    while(more){more--;c=s[i++];if(c<0x80||c>0xbf)return 0;cp=(cp<<6)|(c&0x3f);}
    if(cp<min||cp>0x10ffff||(cp>=0xd800&&cp<=0xdfff)||cp<0x20||(cp>=0x7f&&cp<=0x9f))return 0;
  }
  return 1;
}
static int relay_unhex(char *dst,const char *s) {
  size_t n=strlen(s);if(n<2||n>64||n%2)return 0;
  for(size_t i=0;i<n;i+=2) {
    unsigned v=0;
    for(size_t j=i;j<i+2;j++) {
      char c=s[j];int x=c>='0'&&c<='9'?c-'0':c>='a'&&c<='f'?c-'a'+10:c>='A'&&c<='F'?c-'A'+10:-1;
      if(x<0)return 0;v=v*16+(unsigned)x;
    }
    if(!v)return 0;dst[i/2]=(char)v;
  }
  dst[n/2]=0;return relay_ssid_utf8((const unsigned char*)dst,n/2);
}
static void parse_relay(char **f,int n,struct relay_catalog *r) {
  unsigned long long rev,gen,stamp,ns,nw;
  if(n!=7||!relay_number(f[1],&rev)||!relay_number(f[2],&gen)||!relay_number(f[4],&stamp)||
     !relay_number(f[5],&ns)||!relay_number(f[6],&nw)||ns>8||nw>60)return;
  int state=!strcmp(f[3],"idle")?0:!strcmp(f[3],"running")?1:!strcmp(f[3],"ready")?2:!strcmp(f[3],"failed")?3:-1;
  if(state<0||(state!=2&&nw))return;
  r->has=1;r->rev=rev;r->gen=gen;r->stamp=stamp;r->state=state;r->saved_count=(int)ns;r->scan_count=(int)nw;
}
static void parse_relay_saved(char **f,int n,struct relay_catalog *r) {
  struct relay_saved s={0};
  if(!r->has||n!=7||r->ns>=r->saved_count||!relay_number(f[1],&s.id)||!s.id||
     !relay_bool(f[2])||!relay_bool(f[4])||!relay_bool(f[5])||
     (s.security=relay_security(f[3]))<0||s.security>3||!relay_unhex(s.ssid,f[6]))return;
  for(int i=0;i<r->ns;i++)if(r->saved[i].id==s.id)return;
  s.preferred=atoi(f[2]);s.hidden=atoi(f[4]);s.has_password=atoi(f[5]);r->saved[r->ns++]=s;
}
static void parse_relay_scan(char **f,int n,struct relay_catalog *r) {
  struct relay_scan s={0};unsigned long long index,freq;char *end;
  if(!r->has||r->state!=2||n!=6||r->nw>=r->scan_count||!relay_number(f[1],&index)||index!=(unsigned)r->nw||
     (s.security=relay_security(f[2]))<0||!relay_number(f[4],&freq)||freq>100000||!relay_unhex(s.ssid,f[5]))return;
  long signal=strtol(f[3],&end,10);if(!*f[3]||*end||signal < -150||signal>0)return;
  s.index=(int)index;s.signal=(int)signal;s.frequency=(int)freq;r->scan[r->nw++]=s;
}
/* T/t contain only redacted status and bounded index/name choices. */
static void parse_tailscale(char **f, int n, struct tailscale_info *t) {
  if(n != 8 || (strcmp(f[3],"0") && strcmp(f[3],"1")) ||
     (strcmp(f[5],"0") && strcmp(f[5],"1"))) return;
  if(strcmp(f[4],"Running") && strcmp(f[4],"Stopped") && strcmp(f[4],"Starting") &&
     strcmp(f[4],"NeedsLogin") && strcmp(f[4],"NeedsMachineAuth") && strcmp(f[4],"Unavailable")) return;
  t->has=1;t->rev=u64_field(f[1]);t->gen=u64_field(f[2]);
  t->stamp=u64_field(f[7]);t->enabled=atoi(f[3]);t->error=atoi(f[5]);COPY(t->state,f[4]);
  copy_name(t->selected,sizeof(t->selected),f[6]);
}
static void parse_cfopt(char **f,int n,struct cfopt_info *c) {
  if(n!=12||(strcmp(f[2],"0")&&strcmp(f[2],"1"))||(strcmp(f[3],"0")&&strcmp(f[3],"1")))return;
  c->has=1;c->rev=u64_field(f[1]);c->enabled=atoi(f[2]);c->running=atoi(f[3]);
  COPY(c->phase,f[4]);COPY(c->ip,f[5]);COPY(c->colo,f[6]);
  c->tls_ms=(unsigned)u64_field(f[7]);c->speed_kbps=(unsigned)u64_field(f[8]);
  c->last_run=u64_field(f[9]);c->interval=atoi(f[10]);COPY(c->problem,f[11]);
}
/* h: whether tailnet port 22 forwards to local SSH, and whether that SSH runs. */
static void parse_tailscale_ssh(char **f,int n,struct tailscale_info *t) {
  if(!t->has||n!=3||strlen(f[1])!=1||strlen(f[2])!=1||!strchr("012",f[1][0])||!strchr("012",f[2][0]))return;
  t->has_ssh=1;t->remote_ssh=f[1][0]-'0';t->ssh_service=f[2][0]-'0';
}
static void parse_tailscale_row(char **f,int n,struct tailscale_info *t) {
  if(!t->has || n!=4 || t->n>=32 || u64_field(f[1])!=(unsigned)t->n ||
     (strcmp(f[2],"0") && strcmp(f[2],"1"))) return;
  struct tailscale_row *r=&t->rows[t->n];r->index=t->n++;r->selected=atoi(f[2]);
  copy_name(r->name,sizeof(r->name),f[3]);
}
