/* I1 is private, bounded cached state, never a request to probe the modem. GPL-3.0-or-later. */
struct imei_info {
  int has, busy, allowed, expires;
  unsigned long long rev, stamp;
  char phase[24], code[49], current[2][16], secondary[9], op[33], preview[33], mask[2][16];
};
static int imei_digits(const char *s) {
  if(strlen(s)!=15)return 0;
  for(int i=0;i<15;i++)if(s[i]<'0'||s[i]>'9')return 0;
  return 1;
}
static int imei_luhn(const char *s) {
  if(!imei_digits(s)||!strcmp(s,"000000000000000"))return 0;
  int sum=0;
  for(int i=0;i<15;i++){int d=s[i]-'0';if(i&1){d*=2;if(d>9)d-=9;}sum+=d;}
  return sum%10==0;
}
static int imei_hex(const char *s) {
  if(strlen(s)!=32)return 0;
  for(int i=0;i<32;i++)if(!strchr("0123456789abcdef",s[i]))return 0;
  return 1;
}
static int imei_number(const char *s,unsigned long long max,unsigned long long *out) {
  if(!*s||strlen(s)>16)return 0;
  unsigned long long n=0;
  for(;*s;s++){if(*s<'0'||*s>'9'||n>(max-(unsigned)(*s-'0'))/10)return 0;n=n*10+(unsigned)(*s-'0');}
  if(n>max)return 0;
  *out=n;return 1;
}
static int imei_phase(const char *s) {
  static const char *const phases[]={"idle","checking","checked","preview","preparing","reboot_to_enable","ready_to_write","writing","reboot_to_verify","complete","recovery_required","unsupported","error"};
  for(size_t i=0;i<sizeof(phases)/sizeof(*phases);i++)if(!strcmp(s,phases[i]))return 1;
  return 0;
}
static int imei_code(const char *s) {
  size_t n=strlen(s);if(!n||n>48)return 0;
  for(;*s;s++)if(!(*s>='a'&&*s<='z')&&!(*s>='0'&&*s<='9')&&*s!='_')return 0;
  return 1;
}
static int imei_optional(const char *s,int kind) {
  if(!strcmp(s,"-"))return 1;
  if(kind==0)return imei_digits(s);
  if(kind==1)return imei_hex(s);
  if(strlen(s)!=15||strncmp(s,"***********",11))return 0;
  for(int i=11;i<15;i++)if(s[i]<'0'||s[i]>'9')return 0;
  return 1;
}
static int parse_imei(char *line,struct imei_info *out) {
  struct imei_info v={0};char *f[16];unsigned long long busy,allowed,expires;
  memset(out,0,sizeof(*out));
  if(strlen(line)>511)return 0;
  int n=split_tabs(line,f,16);
  if(n!=15||strcmp(f[0],"I1")||!imei_number(f[1],9007199254740991ULL,&v.rev)||!v.rev||
     !imei_number(f[2],9007199254740991ULL,&v.stamp)||!v.stamp||
     (strcmp(f[3],"0")&&strcmp(f[3],"1"))||!imei_number(f[3],1,&busy)||
     !imei_phase(f[4])||!imei_code(f[5])||!imei_optional(f[6],0)||!imei_optional(f[7],0)||
     (strcmp(f[8],"active")&&strcmp(f[8],"inactive")&&strcmp(f[8],"unknown"))||
     !imei_optional(f[9],1)||!imei_optional(f[10],1)||!imei_optional(f[11],2)||!imei_optional(f[12],2)||
     !imei_number(f[13],511,&allowed)||!imei_number(f[14],60,&expires))return 0;
  v.has=1;v.busy=(int)busy;v.allowed=(int)allowed;v.expires=(int)expires;
  COPY(v.phase,f[4]);COPY(v.code,f[5]);COPY(v.secondary,f[8]);
  if(strcmp(f[6],"-"))COPY(v.current[0],f[6]);
  if(strcmp(f[7],"-"))COPY(v.current[1],f[7]);
  if(strcmp(f[9],"-"))COPY(v.op,f[9]);
  if(strcmp(f[10],"-"))COPY(v.preview,f[10]);
  if(strcmp(f[11],"-"))COPY(v.mask[0],f[11]);
  if(strcmp(f[12],"-"))COPY(v.mask[1],f[12]);
  *out=v;return 1;
}
