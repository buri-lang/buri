function __cmd_x_main$main(){
  const ctx_0=[[],[]];
  const a_1=[1,2];
  const b_2=[a_1[0]+10,a_1[1]+20];
  const $t1=[a_1,b_2,'first'].slice();
  $t1[2]='second';
  $host_HostStdout_println(ctx_0[1],[String(b_2[0]),',',String(b_2[1])]);
  $host_HostStdout_println(ctx_0[1],[$t1[2],' ',String($t1[1][0]-$t1[0][0]+($t1[1][1]-$t1[0][1]))]);
  return [0,0];
}
