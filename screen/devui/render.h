/* DevUI HTML/CSS pages rendered by the upstream litehtml host.
 * Every string is bilingual through L(en, zh); the language follows the web panel.
 * Copyright (c) 2026 ZweiChen. GPL-3.0-or-later. */
static char html[32768];
static size_t html_used;
static void add(const char *fmt, ...) {
  if (html_used >= sizeof(html) - 1) return;
  va_list args;
  va_start(args, fmt);
  int n = vsnprintf(html + html_used, sizeof(html) - html_used, fmt, args);
  va_end(args);
  if (n > 0) html_used += (size_t)n < sizeof(html) - html_used ? (size_t)n : sizeof(html) - html_used - 1;
}
static void escaped(const char *s) {
  for (; *s; s++) {
    switch (*s) {
      case '&': add("&amp;"); break;
      case '<': add("&lt;"); break;
      case '>': add("&gt;"); break;
      case '"': add("&quot;"); break;
      case '\'': add("&#39;"); break;
      default: add("%c", *s);
    }
  }
}
/* Escaped text cut on a UTF-8 boundary with "..." to fit `width` pixels at `size`.
 * Glyph advances add up (Unifont has no kerning), so each character is measured once. */
static void shortened(const char *text, int width, int size) {
  char label[512];snprintf(label,sizeof(label),"%s",text);
  if(html_view_text_width_px(label,size)>width) {
    int room=width-html_view_text_width_px("...",size),used=0;
    size_t n=0;
    while(label[n]) {
      size_t k=n+1;
      while(label[k]&&((unsigned char)label[k]&0xc0)==0x80)k++;
      char glyph[8];size_t len=k-n<sizeof(glyph)-1?k-n:sizeof(glyph)-1;
      memcpy(glyph,label+n,len);glyph[len]=0;
      int w=html_view_text_width_px(glyph,size);
      if(used+w>room)break;
      used+=w;n=k;
    }
    if(n>sizeof(label)-4)n=sizeof(label)-4;
    while(n>0&&((unsigned char)label[n]&0xc0)==0x80)n--;
    strcpy(label+n,"...");
  }
  escaped(label);relay_wipe(label,sizeof(label));
}
static void box(int x, int y, int w, int h, const char *cls, const char *label) {
  add("<div class=\"box %s\" style=\"left:%dpx;top:%dpx;width:%dpx;height:%dpx\">", cls, x,y,w,h);
  escaped(label); add("</div>");
}
/* A one-line box whose text is cut with "..." instead of clipped. */
static void sbox(int x, int y, int w, int h, const char *cls, const char *label) {
  add("<div class=\"box %s\" style=\"left:%dpx;top:%dpx;width:%dpx;height:%dpx\">", cls, x,y,w,h);
  shortened(label,w,16); add("</div>");
}
static void rect(int x, int y, int w, int h, const char *cls, const char *color) {
  add("<div class=\"box %s\" style=\"left:%dpx;top:%dpx;width:%dpx;height:%dpx%s%s\"></div>",cls,x,y,w,h,color?";background:":"",color?color:"");
}
/* CHOSEN marks the current choice of a group of buttons; it is not tappable. */
enum { PLAIN, PRIMARY, DANGER, CHOSEN };
static void linkbox(int x, int y, int w, int h, const char *id, const char *label, int enabled, int tone) {
  add("<a id=\"%s\" href=\"act:%s\" class=\"box btn %s\" style=\"left:%dpx;top:%dpx;width:%dpx;height:%dpx;line-height:%dpx\">",id,enabled&&tone!=CHOSEN?id:"none",
      tone==CHOSEN?"chosen":!enabled?"disabled":tone==PRIMARY?"primary":tone==DANGER?"danger":"",x,y,w,h,h-2);
  shortened(label,w-24,16); add("</a>");
}
/* A tappable list row: bold title, muted detail line and a chevron. */
static void open_row(int x, int y, int w, int h, const char *id, int enabled, const char *extra) {
  add("<a id=\"%s\" href=\"act:%s\" class=\"box row %s\" style=\"left:%dpx;top:%dpx;width:%dpx;height:%dpx\">",id,enabled?id:"none",enabled?extra:"off",x,y,w,h);
}
static void chevron(int h) { add("<div class=\"chev\" style=\"line-height:%dpx\">&#8250;</div>",h-2); }
static void nav_row(int x, int y, int w, int h, const char *id, const char *title, const char *detail, const char *detail_cls, int enabled) {
  /* Rows under 52 px centre their two lines; taller ones keep the stylesheet's. */
  char top[2][32]={"",""};
  if(h<52){snprintf(top[0],sizeof(top[0])," style=\"top:%dpx\"",(h-44)/2);snprintf(top[1],sizeof(top[1])," style=\"top:%dpx\"",(h-44)/2+22);}
  open_row(x,y,w,h,id,enabled,"");
  add("<div class=\"name\"%s>",top[0]);shortened(title,w-50,16);
  add("</div><div class=\"sub %s\"%s>",enabled?detail_cls:"",top[1]);shortened(detail,w-50,16);
  add("</div>");chevron(h);add("</a>");
}
/* Row with a label on the left and a value on the right, vertically centred. */
static void value_row(int x, int y, int w, int h, const char *id, const char *label, const char *value, int enabled) {
  open_row(x,y,w,h,id,enabled,"");
  add("<div class=\"box muted\" style=\"left:14px;top:%dpx;width:%dpx;height:20px\">",(h-22)/2,w/2-14);escaped(label);
  add("</div><div class=\"box head right\" style=\"left:%dpx;top:%dpx;width:%dpx;height:20px\">",w/2-10,(h-22)/2,w/2-24);
  shortened(value,w/2-24,16);add("</div>");chevron(h);add("</a>");
}
static const char *summary(void) {
  return !data.stamp ? L("Loading status","正在加载状态") : stale() ? L("Status is stale","状态已过期") :
    applying(0) ? L("Applying changes","正在应用更改") : !data.has ? L("No configuration","尚未配置") :
    data.error ? L("Tunnel error","隧道错误") : !data.enabled ? L("Off","已关闭") :
    data.connected ? L("Recent handshake","握手正常") : L("Waiting for handshake","等待握手");
}
/* Colour of the summary above: red for trouble, green when connected, amber in between. */
static const char *summary_class(void) {
  return !data.stamp || applying(0) ? "warn" : stale() || data.error ? "bad" :
    !data.has || !data.enabled ? "muted" : data.connected ? "good" : "warn";
}
static const char *tone_color(const char *c) {
  return !strcmp(c,"good") ? "#4cc97a" : !strcmp(c,"bad") ? "#f06a6a" : !strcmp(c,"warn") ? "#eaa646" : "#646c78";
}
static const char *summary_color(void) { return tone_color(summary_class()); }
static const char *active_profile(void) {
  for (int i=0;i<data.np;i++) if(data.profiles[i].active) return data.profiles[i].name;
  return L("None selected","未选择");
}
static int selected_count(void) {
  int n=0;for(int i=0;i<data.n;i++) n+=data.rows[i].selected;return n;
}
/* The title bar: back at the top left, then the page title. Pages start below y 64. */
static void header(const char *title) {
  rect(0,0,W,61,"bar",NULL);
  linkbox(6,6,84,48,"back",L("‹ Back","‹ 返回"),1,PLAIN);
  add("<div class=\"box title\" style=\"left:102px;top:14px;width:206px;height:32px\">");shortened(title,206,24);add("</div>");
}
static void dot(int x, int y, const char *color) {
  add("<div class=\"box\" style=\"left:%dpx;top:%dpx;width:10px;height:10px;border-radius:5px;background:%s\"></div>",x,y,color);
}
static void battery_badge(int x, int y, int large) {
  int percent = data.battery < 0 ? 0 : data.battery > 100 ? 100 : data.battery;
  const char *color = data.battery < 0 ? "#9aa1ac" : percent <= 20 ? "#f06a6a" : "#4cc97a";
  int w=large?62:30, h=large?30:16;
  add("<div class=\"box\" style=\"left:%dpx;top:%dpx;width:%dpx;height:%dpx;border:2px solid %s;border-radius:%dpx\"></div>",x,y,w,h,color,large?6:3);
  add("<div class=\"box\" style=\"left:%dpx;top:%dpx;width:3px;height:%dpx;background:%s\"></div>",x+w+1,y+h/3,h-2*(h/3),color);
  add("<div class=\"box\" style=\"left:%dpx;top:%dpx;width:%dpx;height:%dpx;background:%s;border-radius:%dpx\"></div>",x+4,y+4,(w-8)*percent/100,h-8,color,large?3:1);
  char label[24];
  if(data.battery<0)snprintf(label,sizeof(label),"--%%");else snprintf(label,sizeof(label),"%d%%",percent);
  if(large)box(x+w+14,y-3,120,36,"big",label);else box(x+w+10,y-2,60,20,"head",label);
}
/* Label, right-aligned value and an optional usage bar (percent < 0: no bar). */
static void stat_row(int y, const char *label, const char *value, double percent) {
  box(26,y,130,20,"muted",label);box(156,y,138,20,"head right",value);
  if(percent<0)return;
  int fill=(int)(percent<0?0:percent>100?268:percent*268/100);
  rect(26,y+26,268,6,"bar-track",NULL);
  if(fill>0)rect(26,y+26,fill,6,"fill",percent>=85?"#f06a6a":percent>=60?"#eaa646":"#4c9eeb");
}
/* A section label above a group of controls. */
static void section(int y, const char *text) { sbox(16,y,292,20,"label",text); }
/* A row that turns something on or off: a title, its state and a switch on the right.
 * `id` stays the same whichever way it points; `action` is what a tap does now. */
static void switch_row(int x, int y, int w, int h, const char *id, const char *action, const char *title, const char *detail, int on, int enabled) {
  add("<a id=\"%s\" href=\"act:%s\" class=\"box row%s\" style=\"left:%dpx;top:%dpx;width:%dpx;height:%dpx\">",id,enabled?action:"none",enabled?"":" off",x,y,w,h);
  add("<div class=\"name\" style=\"top:%dpx;right:70px\">",(h-44)/2);shortened(title,w-84,16);
  add("</div><div class=\"sub\" style=\"top:%dpx;right:70px\">",(h-44)/2+22);shortened(detail,w-84,16);add("</div>");
  add("<div class=\"box track%s\" style=\"left:%dpx;top:%dpx;width:44px;height:24px\"></div>",on?" on":"",w-58,(h-24)/2);
  add("<div class=\"box knob\" style=\"left:%dpx;top:%dpx;width:18px;height:18px\"></div></a>",w-58+(on?23:3),(h-24)/2+3);
}
/* Four 48px targets on Overview; unknown is a label, never an OFF switch. */
static void admin_row(int row, int y) {
  const struct admin_info *a=&data.admin;
  const char *title=row==0?L("ADB now","ADB 即时开关"):row==1?L("ADB at boot","ADB 开机自启"):
                    row==2?L("SSH now","SSH 即时开关"):L("SSH at boot","SSH 开机自启");
  char id[16];snprintf(id,sizeof(id),"admin%d",row);
  int available=row<2?a->adb_available:a->ssh_available;
  if(!admin_fresh()||!available) {
    add("<a id=\"%s\" href=\"act:none\" class=\"box row off\" style=\"left:12px;top:%dpx;width:296px;height:48px\">",id,y);
    add("<div class=\"name\" style=\"top:3px;right:12px\">");escaped(title);add("</div>");
    add("<div class=\"sub\" style=\"top:25px;right:12px\">");escaped(!admin_fresh()?L("Unknown / stale","未知 / 已过期"):L("Unavailable","不可用"));add("</div></a>");
    return;
  }
  const char *detail=(row&1)?(admin_value(row)?L("On · next boot only","开 · 仅影响下次启动"):L("Off · next boot only","关 · 仅影响下次启动")):
                           (admin_value(row)?L("On · runtime only","开 · 仅本次运行"):L("Off · runtime only","关 · 仅本次运行"));
  if(row==0&&!a->adb_exposed)detail=a->adb_running?L("Daemon on · USB hidden","进程已开 · USB 未暴露"):L("No ADB USB function","无 ADB USB 功能");
  switch_row(12,y,296,48,id,id,title,detail,admin_value(row),admin_can(row));
}
/* USB modes in the USB page's order; -1 when the active one is none of them. The B31
 * firmware switches only these two (its composition script has no NCM). */
static const char *const usb_modes[]={"rndis","ecm"};
static int usb_mode_index(const char *mode) {
  for(int i=0;i<2;i++)if(!strncmp(mode,usb_modes[i],strlen(usb_modes[i])))return i;
  return -1;
}
/* Overview's USB LAN switch; no record (older agent) or a stale one is a label, not OFF. */
static int usb_lan_can(void) { return !blocked()&&data.toolbox.has&&data.toolbox.has_usb_lan; }
static void usb_lan_row(int y) {
  const struct toolbox_info *h=&data.toolbox;
  const char *title=L("USB LAN","USB 网络共享");
  if(!h->has||!h->has_usb_lan||stale()) {
    add("<a id=\"ulan\" href=\"act:none\" class=\"box row off\" style=\"left:12px;top:%dpx;width:296px;height:48px\">",y);
    add("<div class=\"name\" style=\"top:3px;right:12px\">");escaped(title);add("</div>");
    add("<div class=\"sub\" style=\"top:25px;right:12px\">");escaped(L("Unknown / stale","未知 / 已过期"));add("</div></a>");
    return;
  }
  char detail[96];
  int has_if=h->usb_lan_iface[0]&&strcmp(h->usb_lan_iface,"-");
  if(pending&&pending_kind==JOB_TOOL)snprintf(detail,sizeof(detail),"%s",L("Applying…","正在应用…"));
  else if(!h->usb_lan_on)snprintf(detail,sizeof(detail),"%s",L("Off · not shared over USB","关 · 不向 USB 设备共享网络"));
  else if(!has_if)snprintf(detail,sizeof(detail),"%s",L("On · no USB adapter","开 · 未检测到 USB 网卡"));
  else if(h->usb_lan_state==2)snprintf(detail,sizeof(detail),L("On · %s connected","开 · %s 已连接"),h->usb_lan_iface);
  else if(h->usb_lan_state==1)snprintf(detail,sizeof(detail),L("On · waiting for USB host","开 · 等待 USB 设备连接"));
  else snprintf(detail,sizeof(detail),L("On · %s not up yet","开 · %s 尚未生效"),h->usb_lan_iface);
  switch_row(12,y,296,48,"ulan",h->usb_lan_on?"ulan0":"ulan1",title,detail,h->usb_lan_on,usb_lan_can());
}
static void dialog_card(int y, int h) {
  rect(0,61,W,H-61,"shade",NULL);
  add("<div class=\"box card\" style=\"left:12px;top:%dpx;width:296px;height:%dpx\"></div>",y,h);
}
#include "pages.h"
#include "imei_pages.h"
/* Main menu features: their names (shorter on a tile), marks, row details and the word
 * or two a tile shows. */
static const char *menu_title(int p, int tile) {
  switch(p) {
  case OVERVIEW: return L("Overview","概览");
  case PROXY: return L("Proxy","代理");
  case WG: return "WireGuard";
  case CLASH: return "Clash";
  case TAILSCALE: return "Tailscale";
  case ESIM: return "eSIM";
  case UPLINK: return L("Wi-Fi relay","无线中继");
  case WIFI_PAGE: return "Wi-Fi";
  case SIGNAL_PAGE: return L("Signal","信号");
  case USB_PAGE: return tile?L("USB & TTL","USB/TTL"):L("USB & TTL","USB 与 TTL");
  case POWER_PAGE: return tile?L("Battery","电池"):L("Battery & charge","电池与充电");
  case BOOSTER_PAGE: return tile?L("Booster","加速"):L("Booster","加速清理");
  case PROFILES: return tile?L("Profiles","配置"):L("Saved profiles","已保存配置");
  default: return tile?L("Routing","分流"):L("Device routing","设备分流");
  }
}
static const char *menu_icon(int p) {
  switch(p) {
  case OVERVIEW: return "&#9673;";      /* ◉ */
  case PROXY: return "&#8644;";         /* ⇄ */
  case WG: return "&#9919;";            /* ⚿ */
  case CLASH: return "&#10022;";        /* ✦ */
  case TAILSCALE: return "&#8644;";
  case ESIM: return "&#9635;";          /* ▣ */
  case UPLINK: return "&#8673;";        /* ⇡ */
  case WIFI_PAGE: return "&#8779;";     /* ≋ */
  case SIGNAL_PAGE: return "&#9602;&#9604;&#9606;"; /* ▂▄▆ */
  case USB_PAGE: return "&#8652;";      /* ⇌ */
  case POWER_PAGE: return "&#9889;";    /* ⚡ */
  case BOOSTER_PAGE: return "&#10227;"; /* ⟳ */
  case PROFILES: return "&#9776;";      /* ☰ */
  default: return "&#8862;";            /* ⊞ */
  }
}
static const char *menu_detail(int p) {
  const struct backend_info *bk=&data.backend;
  switch(p) {
  case OVERVIEW: return L("Battery, uptime and load","电池、运行时间与系统负载");
  case PROXY: return !bk->has||bk->active==BK_OFF?L("Proxy modes & Tailscale","代理模式与 Tailscale"):
    bk->active==BK_WG?L("Active: WireGuard","当前：WireGuard"):bk->active==BK_TS?L("Active: Tailscale","当前：Tailscale"):L("Active: Clash","当前：Clash");
  case WG: return L("Tunnel status and on / off","隧道状态与开关");
  case CLASH: return L("Nodes, subscriptions and core","节点、订阅与内核");
  case TAILSCALE: return ts_summary();
  case ESIM: return esim_summary();
  case UPLINK: return uplink_summary();
  case WIFI_PAGE: return L("Bands and connected clients","频段开关与在线终端");
  case SIGNAL_PAGE: return L("Signal and network mode","信号与网络制式");
  case USB_PAGE: return L("USB mode and TTL","USB 模式与 TTL");
  case POWER_PAGE: return L("Charging and limit","充电开关与上限");
  case BOOSTER_PAGE: return L("Free memory","清理内存与冗余进程");
  case PROFILES: return L("Switch between up to 5 configs","在最多 5 个配置之间切换");
  default: return L("Choose WireGuard or direct","选择走 WireGuard 或直连");
  }
}
static void tile_state(int p, char *buf, size_t n, const char **cls) {
  const struct toolbox_info *h=&data.toolbox;
  const struct backend_info *bk=&data.backend;
  *cls="muted";
  switch(p) {
  case OVERVIEW: snprintf(buf,n,"CPU %.0f%%",data.cpu); return;
  case PROXY:
    *cls=bk->has?proxy_class():"muted";
    snprintf(buf,n,"%s",!bk->has||bk->active==BK_OFF?L("Off","关闭"):bk->active==BK_WG?"WireGuard":bk->active==BK_TS?"Tailscale":"Clash"); return;
  case WG: *cls=summary_class(); snprintf(buf,n,"%s",summary()); return;
  case CLASH: *cls=data.clash.has?clash_class():"muted"; snprintf(buf,n,"%s",data.clash.has?clash_summary():"--"); return;
  case TAILSCALE: snprintf(buf,n,"%s",ts_summary()); return;
  case ESIM:
    if(data.esim.has&&!strcmp(data.esim.card,"euicc"))snprintf(buf,n,"%s",esim_summary());
    else snprintf(buf,n,"%s",data.esim.has&&!strcmp(data.esim.card,"sim")?L("SIM card","普通 SIM"):"--");
    return;
  case UPLINK: *cls=uplink_class(); snprintf(buf,n,"%s",data.uplink.has?uplink_state_text(data.uplink.state):"--"); return;
  case WIFI_PAGE:
    if(!h->wifi_2g_on&&!h->wifi_5g_on){*cls="warn";snprintf(buf,n,"%s",L("Off","已关闭"));}
    else snprintf(buf,n,L("%llu online","%llu 台在线"),h->wifi_clients);
    return;
  case SIGNAL_PAGE:
    if(!h->has||h->rsrp==NO_FIGURE){snprintf(buf,n,"--");return;}
    *cls=h->rsrp>-95?"good":h->rsrp>-110?"warn":"bad";snprintf(buf,n,"%lld dBm",h->rsrp); return;
  case USB_PAGE:
    if(h->ttl_val)snprintf(buf,n,"TTL %llu",h->ttl_val);
    else snprintf(buf,n,"%s",h->usb_mode[0]?h->usb_mode:"--");
    return;
  case POWER_PAGE:
    if(!h->chg_enabled){*cls="warn";snprintf(buf,n,"%s",L("Paused","已停充"));}
    else if(h->chg_limit_on)snprintf(buf,n,L("Limit %llu%%","上限 %llu%%"),h->chg_stop);
    else if(data.battery>=0)snprintf(buf,n,"%d%%",data.battery);
    else snprintf(buf,n,"--");
    return;
  case BOOSTER_PAGE: snprintf(buf,n,L("RAM %.0f%%","内存 %.0f%%"),data.memory>=0?data.memory:0.0); return;
  case PROFILES: snprintf(buf,n,"%s",active_profile()); return;
  default: snprintf(buf,n,"%d / %d",selected_count(),data.n); return;
  }
}
static uint16_t pixels[W * H];
/* The current page as HTML. Cheap next to laying it out and drawing it. */
static void build_html(void) {
  html_used=0;relay_wipe(html,sizeof(html));
  add("<!DOCTYPE html><html lang=\"%s\"><head><meta charset=\"UTF-8\"><style>%s</style></head><body>",L("en","zh"),style_asset);
  char tmp[256],rx[32],tx[32];
  if (dialog>=DLG_POWER) render_power_dialog();
  else if (dialog) {
    header(page_title());
    if (dialog==5) render_confirm();
    else if (dialog==6) render_sub_dialog();
    else if (dialog==1 || dialog==4) {
      dialog_card(112,244);
      box(28,128,264,32,"title",dialog==1?L("Disable WireGuard?","关闭 WireGuard？"):L("Switch profile?","切换配置？"));
      box(28,170,264,60,"wrap",dialog==1?L("Devices will return to the normal network.","设备将恢复为普通网络。"):
          data.enabled?L("Tunnel traffic will briefly pause. Routing choices stay saved.","隧道流量会短暂中断，设备分流选择保持不变。"):
          L("Select this profile. WireGuard will remain off.","选中此配置，WireGuard 仍保持关闭。"));
      if(dialog==4) { add("<div class=\"box head accent\" style=\"left:28px;top:236px;width:264px;height:20px\">");shortened(chosen_name,264,16);add("</div>"); }
      linkbox(24,284,128,56,"dismiss",L("Cancel","取消"),1,PLAIN);
      linkbox(168,284,128,56,"confirm",dialog==1?L("Disable","关闭"):L("Switch","切换"),!blocked(),dialog==1?DANGER:PRIMARY);
    } else if(dialog==2) {
      dialog_card(88,332);
      box(28,106,264,32,"title",L("Unsaved changes","有未保存的更改"));
      box(28,146,264,40,"wrap muted",L("Device routing choices have not been saved.","设备分流的选择尚未保存。"));
      linkbox(24,200,272,56,"saveback",L("Save and return","保存并返回"),!blocked(),PRIMARY);
      linkbox(24,268,272,56,"discard",L("Discard changes","放弃更改"),1,DANGER);
      linkbox(24,336,272,56,"dismiss",L("Keep editing","继续编辑"),1,PLAIN);
    } else {
      dialog_card(112,244);
      box(28,128,264,32,"title bad",L("Action failed","操作失败"));
      box(28,170,264,40,"wrap",notice);
      /* The agent's own message, when it gave one (English). */
      box(28,214,264,60,"wrap muted",failure_text[0]?failure_text:L("Reload status before trying again.","请先刷新状态再重试。"));
      /* WireGuard pages go back to WireGuard; the others reload where they are. */
      linkbox(24,284,272,56,"refresh",page==WG||page==PROFILES||page==DEVICES?L("Refresh and back","刷新并返回"):L("Refresh","刷新"),1,PRIMARY);
    }
  } else if(page==MENU) {
    box(16,14,170,32,"title","OpenUI");
    linkbox(196,8,116,48,"stock",L("Stock UI","原厂界面"),1,PLAIN);
    /* Status card: battery and signal, then the proxy and the current speed. */
    const struct backend_info *bk=&data.backend;
    const struct toolbox_info *tb=&data.toolbox;
    add("<div class=\"box card\" style=\"left:10px;top:64px;width:300px;height:84px\"></div>");
    battery_badge(24,78,0);
    if(tb->has&&tb->rsrp!=NO_FIGURE) {
      snprintf(tmp,sizeof(tmp),"%s %lld dBm",tb->radio[0]?tb->radio:"5G",tb->rsrp);
      snprintf(rx,sizeof(rx),"head right %s",tb->rsrp>-95?"good":tb->rsrp>-110?"warn":"bad");
      sbox(130,76,166,20,rx,tmp);
    } else if(data.uptime) {
      if(data.uptime>=86400)snprintf(tmp,sizeof(tmp),L("Up %llud %lluh","已运行 %llu 天 %llu 时"),data.uptime/86400,data.uptime/3600%24);
      else snprintf(tmp,sizeof(tmp),L("Up %lluh %llum","已运行 %llu 时 %llu 分"),data.uptime/3600,data.uptime/60%60);
      sbox(130,76,166,20,"muted right",tmp);
    }
    rect(22,105,276,1,"line",NULL);
    /* The active proxy backend when the agent reports it, else WireGuard as before. */
    const char *cls=bk->has?proxy_class():summary_class();
    dot(24,121,tone_color(cls));
    /* The backend's name first when the line has room for both. */
    int room=tb->has?148:254;
    if(bk->has&&bk->active==BK_OFF)snprintf(tmp,sizeof(tmp),"%s",proxy_summary());
    else {
      snprintf(tmp,sizeof(tmp),"%s · %s",bk->has&&bk->active==BK_CLASH?"Clash":bk->has&&bk->active==BK_TS?"Tailscale":"WireGuard",bk->has?proxy_summary():summary());
      if(html_view_text_width_px(tmp,16)>room)snprintf(tmp,sizeof(tmp),"%s",bk->has&&bk->active==BK_TS?"Tailscale":bk->has?proxy_summary():summary());
    }
    snprintf(rx,sizeof(rx),"head %s",strcmp(cls,"muted")?cls:"");
    if(tb->has) {
      char d_rate[24],u_rate[24],rates[64];
      format_rate(d_rate,sizeof(d_rate),tb->kbps_down);format_rate(u_rate,sizeof(u_rate),tb->kbps_up);
      sbox(42,116,room,20,rx,tmp);
      snprintf(rates,sizeof(rates),"↓%s ↑%s",d_rate,u_rate);
      sbox(190,116,106,20,"muted right",rates);
    } else sbox(42,116,room,20,rx,tmp);
    const int pages[] = {OVERVIEW,UPLINK,PROXY,WG,CLASH,ESIM,WIFI_PAGE,SIGNAL_PAGE,USB_PAGE,POWER_PAGE,BOOSTER_PAGE,TAILSCALE,PROFILES,DEVICES};
    const char *ids[] = {"overview","uplink","proxy","wireguard","clash","esim","wifi","signal","usb","power","booster","tailscale","profiles","devices"};
    int vis_idx[14], vis_cnt=0;
    for(int i=0;i<14;i++) if(visible_page(pages[i])) vis_idx[vis_cnt++]=i;
    if(vis_cnt<=4) {
      for(int r=0;r<vis_cnt;r++) {
        int i=vis_idx[r];
        nav_row(10,160+64*r,300,58,ids[i],menu_title(pages[i],0),menu_detail(pages[i]),"",1);
      }
    } else {
      /* Up to nine tiles: a mark, the feature and its state in a word or two. */
      int show=vis_cnt>9?9:vis_cnt;
      for(int r=0;r<show;r++) {
        int i=vis_idx[r],x=10+(r%3)*102,y=158+(r/3)*86;
        const char *scls="muted";
        tile_state(pages[i],tmp,sizeof(tmp),&scls);
        add("<a id=\"%s\" href=\"act:%s\" class=\"box tile\" style=\"left:%dpx;top:%dpx;width:96px;height:80px\">",ids[i],ids[i],x,y);
        add("<div class=\"box icon\" style=\"left:10px;top:9px;width:76px;height:20px\">%s</div>",menu_icon(pages[i]));
        add("<div class=\"box head\" style=\"left:10px;top:31px;width:78px;height:20px\">");shortened(menu_title(pages[i],1),78,16);
        add("</div><div class=\"box %s\" style=\"left:10px;top:53px;width:78px;height:20px\">",scls);shortened(tmp,78,16);add("</div></a>");
      }
    }
    box(16,424,292,20,"muted",L("Power: tap to sleep, hold for menu.","电源键：短按息屏，长按关机或重启"));
    if(settings_sleep) snprintf(tmp,sizeof(tmp),L("Screen turns off after %d s idle.","空闲 %d 秒后熄屏"),settings_idle);
    else snprintf(tmp,sizeof(tmp),L("Stock UI returns after %d s idle.","空闲 %d 秒后返回原厂界面"),settings_idle);
    box(16,446,292,20,"faint",tmp);
  } else if(page==WG) {
    header("WireGuard");
    add("<div class=\"box card\" style=\"left:12px;top:76px;width:296px;height:92px\"></div>");
    dot(28,91,summary_color());
    snprintf(tmp,sizeof(tmp),"head %s",strcmp(summary_class(),"muted")?summary_class():"");
    box(46,86,250,20,tmp,summary());
    if(!data.has)snprintf(tmp,sizeof(tmp),"%s",L("Import a config in OpenUI","请先在 OpenUI 中导入配置"));
    else if(data.enabled&&data.handshake)snprintf(tmp,sizeof(tmp),L("Last handshake: %llu s ago","上次握手：%llu 秒前"),(unsigned long long)time(NULL)>data.handshake?(unsigned long long)time(NULL)-data.handshake:0);
    else snprintf(tmp,sizeof(tmp),"%s",data.enabled?L("Waiting for the server","正在等待服务器响应"):L("Devices use normal internet","设备使用普通网络"));
    box(28,112,268,20,"muted",tmp);
    box(28,136,268,20,"muted",L("Internet access not verified","未验证互联网连通性"));
    amount(rx,sizeof(rx),data.rx);amount(tx,sizeof(tx),data.tx);
    add("<div class=\"box card\" style=\"left:12px;top:180px;width:142px;height:64px\"></div><div class=\"box card\" style=\"left:166px;top:180px;width:142px;height:64px\"></div>");
    box(26,188,120,20,"muted",L("↓ Downloaded","↓ 下载"));box(180,188,120,20,"muted",L("↑ Uploaded","↑ 上传"));
    box(26,208,124,32,"title",rx);box(180,208,124,32,"title",tx);
    linkbox(12,256,296,56,"toggle",data.enabled?L("Disable WireGuard","关闭 WireGuard"):
            data.backend.has&&data.backend.active==BK_CLASH?L("Switch to WireGuard","切换到 WireGuard"):L("Enable WireGuard","开启 WireGuard"),
            !blocked()&&data.has,data.enabled?DANGER:PRIMARY);
    value_row(12,324,296,50,"profiles",L("Profile","当前配置"),active_profile(),reachable(PROFILES));
    snprintf(tmp,sizeof(tmp),"%d / %d",selected_count(),data.n);
    value_row(12,382,296,50,"devices",L("Device routing","设备分流"),tmp,reachable(DEVICES));
    if(notice[0])box(16,446,292,20,"bad",notice);
    else box(16,446,292,20,"muted",data.all?L("New devices: WireGuard","新设备：走 WireGuard"):L("New devices: normal network","新设备：走普通网络"));
  } else if(page==PROFILES) {
    header(L("Saved profiles","已保存配置"));
    snprintf(tmp,sizeof(tmp),L("%d / 5 saved - tap to switch","已保存 %d / 5 个，点击切换"),data.np);
    box(16,76,292,20,"muted",tmp);
    for(int i=0;i<data.np;i++) {
      char id[16];snprintf(id,sizeof(id),"p%d",i);
      int active=data.profiles[i].active;
      open_row(12,104+i*60,296,52,id,active||!blocked(),active?"current":"");
      add("<div class=\"box head\" style=\"left:14px;top:15px;width:%dpx;height:20px\">",active?200:236);shortened(data.profiles[i].name,active?200:236,16);add("</div>");
      if(active)add("<div class=\"box badge\" style=\"right:12px;top:14px;width:56px;height:22px\">%s</div>",L("Active","当前"));
      else chevron(52);
      add("</a>");
    }
    if(!data.np)box(28,150,264,60,"wrap muted",L("Import profiles in the OpenUI webpage first.","请先在 OpenUI 网页中导入配置。"));
    if(stale())box(16,420,292,40,"wrap bad",L("Status is stale. Controls are disabled.","状态已过期，控制已停用。"));
    else {
      box(16,420,292,20,"muted",L("Switching keeps device routing.","切换配置不会改变设备分流。"));
      box(16,442,292,20,"muted",L("Manage names and keys in OpenUI.","名称与密钥请在 OpenUI 中管理。"));
    }
  } else if(page==DEVICES) {
    header(L("Device routing","设备分流"));
    box(16,72,292,20,"muted",draft.all?L("New devices: WireGuard","新设备：走 WireGuard"):L("New devices: normal network","新设备：走普通网络"));
    box(16,92,292,20,stale()?"bad":"muted",stale()?L("Status is stale; changes disabled.","状态已过期，无法修改。"):
        data.enabled?L("Checked: WireGuard. Others: direct.","勾选的走 WireGuard，其余直连。"):L("Changes take effect when WG is on.","WireGuard 开启后生效。"));
    for(int i=0;i<4&&offset+i<draft.n;i++) {
      struct row *r=&draft.rows[offset+i];
      int dup=!strcmp(r->name,"-");
      for(int j=0;j<draft.n;j++)if(j!=offset+i&&!strcmp(r->name,draft.rows[j].name))dup=1;
      int y=120+i*60;
      add("<a id=\"d%d\" href=\"act:d%d\" class=\"box row%s\" style=\"left:12px;top:%dpx;width:296px;height:54px\">",i,i,r->selected?" current":"",y);
      add("<div class=\"box check%s\" style=\"left:12px;top:15px;width:24px;height:24px\">%s</div>",r->selected?" on":"",r->selected?"&#10003;":"");
      add("<div class=\"name\" style=\"left:48px;right:10px;top:7px\">");
      shortened(!strcmp(r->name,"-")?L("Unknown device","未知设备"):r->name,234,16);
      add("</div><div class=\"sub\" style=\"left:48px;right:10px;top:29px\"><span class=\"%s\">&#9679;</span> ",r->online?"good":"muted");
      snprintf(tmp,sizeof(tmp),"%s  %s%s%s",r->online?L("Online","在线"):L("Offline","离线"),strcmp(r->ip,"-")?r->ip:"",dup?"  ":"",dup?r->mac+9:"");
      escaped(tmp);add("</div></a>");
    }
    if(!draft.n)box(28,180,264,40,"wrap muted center",L("No devices found","未发现设备"));
    linkbox(12,364,100,48,"prev",L("‹ Prev","‹ 上页"),offset>0,PLAIN);
    linkbox(208,364,100,48,"next",L("Next ›","下页 ›"),offset+4<draft.n,PLAIN);
    snprintf(tmp,sizeof(tmp),"%d / %d",offset/4+1,draft.n?(draft.n+3)/4:1);box(112,378,96,20,"muted center",tmp);
    linkbox(12,420,142,52,"back",L("Cancel","取消"),1,PLAIN);
    linkbox(166,420,142,52,"save",pending?L("Applying","正在应用"):L("Save","保存"),dirty&&!blocked(),PRIMARY);
  } else if(page==OVERVIEW) {
    header(L("Overview","概览"));
    /* Compact cards leave y=214..262 for USB LAN; the four admin rows keep their places. */
    add("<div class=\"box card\" style=\"left:12px;top:68px;width:296px;height:48px\"></div>");
    battery_badge(28,77,1);
    if(data.temperature!=-999)snprintf(tmp,sizeof(tmp),"%s · %.1f °C",L("Battery","电池"),data.temperature/10.0);
    else snprintf(tmp,sizeof(tmp),"%s · -- °C",L("Battery","电池"));
    box(160,82,134,20,"muted right",tmp);
    add("<div class=\"box card\" style=\"left:12px;top:122px;width:296px;height:86px\"></div>");
    if(data.uptime>=86400)snprintf(tmp,sizeof(tmp),L("%llud %lluh %llum","%llu 天 %llu 时 %llu 分"),data.uptime/86400,data.uptime/3600%24,data.uptime/60%60);
    else snprintf(tmp,sizeof(tmp),L("%lluh %llum","%llu 时 %llu 分"),data.uptime/3600,data.uptime/60%60);
    stat_row(130,L("Uptime","运行时间"),tmp,-1);
    snprintf(tmp,sizeof(tmp),"%.1f%%",data.cpu);stat_row(156,"CPU",tmp,-1);
    if(data.memory>=0)snprintf(tmp,sizeof(tmp),"%.1f%%",data.memory);else strcpy(tmp,"--");
    stat_row(182,L("Memory","内存"),tmp,-1);
    usb_lan_row(214);
    /* Proxy and relay remain independent menu pages, not duplicate Overview links. */
    for(int row=0;row<4;row++)admin_row(row,264+row*48);
    const char *status=!admin_fresh()||stale()?L("Service status unavailable / stale","服务状态不可用或已过期"):
      pending||data.admin.busy?L("Applying service change…","正在应用服务更改…"):
      !data.admin.adb_exposed?L("ADB USB not exposed · no USB switch","ADB USB 未暴露 · 不切换 USB"):
      L("Boot toggles affect next boot only","自启开关仅影响下次启动");
    sbox(16,460,292,18,!admin_fresh()||stale()?"bad":"faint",status);
  }
  else if(page==PROXY) render_proxy();
  else if(page==CLASH) render_clash();
  else if(page==NODES) render_nodes();
  else if(page==GROUP) render_group();
  else if(page==SUBS) render_subs();
  else if(page==CORE) render_core();
  else if(page==DIAG) render_diag();
  else if(page==TAILSCALE) render_tailscale();
  else if(page==ESIM) render_esim();
  else if(page==UPLINK) render_uplink();
  else if(page==WIFI_PAGE) render_wifi();
  else if(page==SIGNAL_PAGE) render_signal();
  else if(page==IMEI_PAGE) render_imei();
  else if(page==USB_PAGE) render_usb();
  else if(page==POWER_PAGE) render_power();
  else if(page==BOOSTER_PAGE) render_booster();
  add("</body></html>");
}
/* The last HTML draw, plus a keyboard-only structural cache. No credential is cached:
 * the scaffold contains anchors/labels, while value/count live only in editor + pixels. */
static char drawn_html[sizeof html];
static int drawn_valid;
static struct {
  int valid,key,mode,show,zh;
  unsigned long long epoch,revision;
} relay_keyboard_cache;
/* Non-secret work counters for preview tests and the native synthetic benchmark. */
static unsigned long long relay_keyboard_scaffold_draws,relay_keyboard_dynamic_draws;
static int relay_keyboard_active(void) {
  return page==UPLINK&&relay_view==RV_KEYBOARD&&relay_edit.active&&!dialog;
}
static int relay_keyboard_same_scaffold(void) {
  return relay_keyboard_cache.valid&&relay_keyboard_cache.epoch==relay_key_epoch&&
    relay_keyboard_cache.key==relay_edit.key&&relay_keyboard_cache.mode==relay_edit.keymode&&
    relay_keyboard_cache.show==relay_edit.show&&relay_keyboard_cache.zh==settings_zh;
}
static void copy_pixels(struct drm_buf *b) {
  for(int y=0;y<H;y++)memcpy((uint8_t*)b->map+y*b->pitch,pixels+y*W,W*2);
}
static void draw_html(struct drm_buf *b) {
  static int initialized;
  if (!initialized) {
    html_view_init(pixels,W,H,W,0,NULL);
    initialized=1;
  }
  /* Any full render replaces the hit document and clears the logical pixels. */
  relay_keyboard_cache.valid=0;drawn_valid=0;
  int ok=html_view_render_to(pixels,html)>=0;
  if(relay_keyboard_active()) {
    relay_keyboard_paint(pixels);relay_keyboard_scaffold_draws++;
    relay_keyboard_cache.valid=ok;relay_keyboard_cache.epoch=relay_key_epoch;
    relay_keyboard_cache.key=relay_edit.key;relay_keyboard_cache.mode=relay_edit.keymode;
    relay_keyboard_cache.show=relay_edit.show;relay_keyboard_cache.zh=settings_zh;
    relay_keyboard_cache.revision=relay_key_revision;
  }
  copy_pixels(b);
}
static void render(struct drm_buf *b) {
  build_html();
  draw_html(b);
}
/* -1: needs a full scaffold; 0: unchanged; 1: text repainted. This runs BEFORE
 * build_html, avoiding repeated CSS/HTML parsing, layout and static glyph work. */
static int relay_keyboard_render_cached(struct drm_buf *b,int force) {
  if(!relay_keyboard_active()||force||!relay_keyboard_same_scaffold())return -1;
  if(relay_keyboard_cache.revision==relay_key_revision)return 0;
  relay_keyboard_paint(pixels);copy_pixels(b);
  relay_keyboard_cache.revision=relay_key_revision;relay_keyboard_dynamic_draws++;
  return 1;
}
/* Equal HTML skips ordinary pages. Keyboard edit revision, not its static HTML,
 * controls text repaint; structural changes, forced frames and dialogs rebuild. */
static int render_changed(struct drm_buf *b, int force) {
  int keyboard=relay_keyboard_render_cached(b,force);
  if(keyboard>=0)return keyboard;
  build_html();
  if (!force && !relay_keyboard_active() && drawn_valid && !strcmp(html,drawn_html)) return 0;
  draw_html(b);
  relay_wipe(drawn_html,sizeof(drawn_html));memcpy(drawn_html,html,html_used+1);
  drawn_valid=1;
  return 1;
}
