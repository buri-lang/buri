const $k0=[3n,1n,4n,1n,5n];
const $k1=[0,0];
function __cmd_x_main_buri$main(){
  const ctx_0=[[],[]];
  const text_3=$str(__cmd_x_main_buri$allBelow($k0,10n,0n))+' '+$str(__cmd_x_main_buri$allBelow($k0,4n,0n));
  const self_4=$host_HostStdout_println(ctx_0[1],text_3);
  let $t1;
  if(self_4[0]===0){
    $t1=0;
  }else if(self_4[0]===1){
    $t1=0;
  }else{
    $abort('no arm matched');
  }
  const text_8=$str(__cmd_x_main_buri$anyAtLeast($k0,5n,0n))+' '+$str(__cmd_x_main_buri$anyAtLeast($k0,9n,0n));
  const self_9=$host_HostStdout_println(ctx_0[1],text_8);
  let $t3;
  if(self_9[0]===0){
    $t3=0;
  }else if(self_9[0]===1){
    $t3=0;
  }else{
    $abort('no arm matched');
  }
  const text_13=$str(__cmd_x_main_buri$bothSmall(1n,2n))+' '+$str(__cmd_x_main_buri$bothSmall(1n,20n));
  const self_14=$host_HostStdout_println(ctx_0[1],text_13);
  let $t5;
  if(self_14[0]===0){
    $t5=0;
  }else if(self_14[0]===1){
    $t5=0;
  }else{
    $abort('no arm matched');
  }
  return $k1;
}
function __cmd_x_main_buri$allBelow(xs_0,limit_1,i_2){
  while(true){
    if(i_2>=$list_len(xs_0)){
      return true;
    }else{
      const self_3=$list_get(xs_0,i_2);
      let $t1;
      if(self_3!==void 0){
        $t1=self_3;
      }else if(self_3===void 0){
        $t1=0n;
      }else{
        $abort('no arm matched');
      }
      if($t1<limit_1){
        i_2=i_2+1n;
        continue;
      }else{
        return false;
      }
    }
  }
}
function __cmd_x_main_buri$anyAtLeast(xs_0,limit_1,i_2){
  while(true){
    if(i_2>=$list_len(xs_0)){
      return false;
    }else{
      const self_3=$list_get(xs_0,i_2);
      let $t1;
      if(self_3!==void 0){
        $t1=self_3;
      }else if(self_3===void 0){
        $t1=0n;
      }else{
        $abort('no arm matched');
      }
      if($t1>=limit_1){
        return true;
      }else{
        i_2=i_2+1n;
        continue;
      }
    }
  }
}
function __cmd_x_main_buri$bothSmall(a_0,b_1){
  return a_0<10n&&b_1<10n;
}
