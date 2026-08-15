function __cmd_x_main$main(){
  const ctx_0=[[],[]];
  const a_1=[3,'default'];
  $host_HostStdout_println(ctx_0[1],[String(a_1[0]+3),' ',a_1[1]]);
  let $t1;
  const $t2=[1];
  if($t2[0]===0){
    $t1=0;
  }else{
    switch($t2[0]){
      case 1:
        {
          $t1=1;
        }
        break;
      case 2:
        {
          $t1=2;
        }
        break;
      case 3:
        {
          $t1=3;
        }
        break;
      default:
        {
          $abort('no arm matched');
        }
        break;
    }
  }
  let $t3;
  const $t4=[3,'x'];
  if($t4[0]===0){
    $t3=0;
  }else{
    switch($t4[0]){
      case 1:
        {
          $t3=1;
        }
        break;
      case 2:
        {
          $t3=2;
        }
        break;
      case 3:
        {
          $t3=3;
        }
        break;
      default:
        {
          $abort('no arm matched');
        }
        break;
    }
  }
  $host_HostStdout_println(ctx_0[1],[String($t1),' ',String($t3)]);
  $host_HostStdout_println(ctx_0[1],[String(0),' ',String($list_len([2,3,5,7,11])),' ',String($list_fold([2,3,5,7,11],(acc_4,x_5)=>acc_4+x_5,0))]);
  return [0,0];
}
