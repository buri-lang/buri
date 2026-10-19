const $k0=[[],[],[],[],[],[],[],[],[],[],[],[],[],[],[]];
const $k1=[0,0];
function __cmd_x_main_buri$main(){
  const text_3=String(__cmd_x_main_buri$next(3n))+' '+String(__cmd_x_main_buri$next(41n));
  const self_4=$host_HostStdout_println($k0[1],text_3);
  let $t1;
  if(self_4[0]===0){
    $t1=0;
  }else if(self_4[0]===1){
    $t1=0;
  }else{
    $abort('no arm matched');
  }
  return $k1;
}
function __cmd_x_main_buri$next(n_0){
  (x_1=>{
    const y_2=x_1+n_0;
    return y_2*y_2+y_2;
  });
  return n_0+1n;
}
