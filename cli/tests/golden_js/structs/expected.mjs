function __cmd_x_main$main(){
  const ctx_0=[[],[]];
  const a_1=[1,2];
  const b_2=__cmd_x_main$moved(a_1,10,20);
  const m_4=__cmd_x_main$relabelled([a_1,b_2,'first'],'second');
  core_host$HostStdout_println(ctx_0[1],[String(b_2[0]),',',String(b_2[1])]);
  core_host$HostStdout_println(ctx_0[1],[m_4[2],' ',String(__cmd_x_main$span(m_4))]);
  return [0,0];
}
function __cmd_x_main$moved(p_0,dx_1,dy_2){
  return [p_0[0]+dx_1,p_0[1]+dy_2];
}
function __cmd_x_main$relabelled(l_0,label_1){
  const $t1=l_0.slice();
  $t1[2]=label_1;
  return $t1;
}
function core_host$HostStdout_println(self_0,text_1){
  return $host_HostStdout_println(self_0,text_1);
}
function __cmd_x_main$span(l_0){
  return l_0[1][0]-l_0[0][0]+(l_0[1][1]-l_0[0][1]);
}
