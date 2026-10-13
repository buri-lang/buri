const $k0=[[],[],[],[],[],[],[],[],[],[],[],[],[],[],[]];
const $k1=[1n,2n];
const $k2=[0,0];
function __cmd_x_main_buri$main(){
  const ctx_1=[$k0[0],$k0[1]];
  const b_3=[11n,22n];
  const $t1=[$k1,b_3,'first'];
  const m_5=[$fromShared($t1,$t1[0]),$fromShared($t1,$t1[1]),'second'];
  const text_12=String(b_3[0])+','+String(b_3[1]);
  const self_13=$host_HostStdout_println(ctx_1[1],text_12);
  let $t2;
  if(self_13[0]===0){
    $t2=0;
  }else if(self_13[0]===1){
    $t2=0;
  }else{
    $abort('no arm matched');
  }
  const text_18=m_5[2]+' '+String(m_5[1][0]-m_5[0][0]+(m_5[1][1]-m_5[0][1]));
  const self_19=$host_HostStdout_println(ctx_1[1],text_18);
  let $t4;
  if(self_19[0]===0){
    $t4=0;
  }else if(self_19[0]===1){
    $t4=0;
  }else{
    $abort('no arm matched');
  }
  return $k2;
}
