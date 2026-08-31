const $k0=[0,0];
function __cmd_x_main_buri$main(){
  const ctx_0=[[],[]];
  const text_2=__cmd_x_main_buri$name(0)+' '+__cmd_x_main_buri$name(5);
  const self_3=$host_HostStdout_println(ctx_0[1],text_2);
  let $t1;
  if(self_3[0]===0){
    $t1=0;
  }else if(self_3[0]===1){
    $t1=0;
  }else{
    $abort('no arm matched');
  }
  const text_7=__cmd_x_main_buri$name(2)+' '+__cmd_x_main_buri$name(4);
  const self_8=$host_HostStdout_println(ctx_0[1],text_7);
  let $t3;
  if(self_8[0]===0){
    $t3=0;
  }else if(self_8[0]===1){
    $t3=0;
  }else{
    $abort('no arm matched');
  }
  return $k0;
}
function __cmd_x_main_buri$name(c_0){
  switch(c_0){
    case 0:
      {
        return 'red';
      }
    case 1:
      {
        return 'green';
      }
    case 2:
      {
        return 'blue';
      }
    case 3:
      {
        return 'cyan';
      }
    case 4:
      {
        return 'magenta';
      }
    case 5:
      {
        return 'yellow';
      }
    default:
      {
        $abort('no arm matched');
      }
      break;
  }
}
