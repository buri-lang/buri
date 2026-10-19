const $k0=[[],[],[],[],[],[],[],[],[],[],[],[],[],[],[]];
const $k1=[0,0];
function __cmd_x_main_buri$main(){
  const ctx_1=[$k0[0],$k0[1]];
  const xs_6=__cmd_x_main_buri$upTo$l3wgis(ctx_1,3n,[]);
  let $t1;
  if(xs_6.length>=1){
    const head_7=xs_6[0];
    const turned_8=$list_reverse(xs_6,ctx_1);
    const text_24=String($list_length(turned_8));
    const self_25=$host_HostStdout_println(ctx_1[1],text_24);
    let $t3;
    if(self_25[0]===0){
      $t3=0;
    }else if(self_25[0]===1){
      $t3=0;
    }else{
      $abort('no arm matched');
    }
    $t1=head_7;
  }else if(xs_6.length===0){
    $t1=0n;
  }else{
    $abort('no arm matched');
  }
  const head_2=$t1;
  const xs_10=__cmd_x_main_buri$upTo$l3wgis(ctx_1,3n,[]);
  let $t5;
  if(xs_10.length>=2){
    const a_11=xs_10[0];
    const b_12=xs_10[1];
    const rest_13=$list_drop(xs_10,ctx_1,1n);
    const text_29=String($list_length(rest_13));
    const self_30=$host_HostStdout_println(ctx_1[1],text_29);
    let $t7;
    if(self_30[0]===0){
      $t7=0;
    }else if(self_30[0]===1){
      $t7=0;
    }else{
      $abort('no arm matched');
    }
    $t5=a_11*10n+b_12;
  }else{
    $t5=0n;
  }
  const pair_3=$t5;
  const xs_15=__cmd_x_main_buri$upTo$l3wgis(ctx_1,3n,[]);
  let $t9;
  if(xs_15.length===3){
    const c_16=xs_15[2];
    const kept_17=$list_take(xs_15,ctx_1,1n);
    const text_34=String($list_length(kept_17));
    const self_35=$host_HostStdout_println(ctx_1[1],text_34);
    let $t11;
    if(self_35[0]===0){
      $t11=0;
    }else if(self_35[0]===1){
      $t11=0;
    }else{
      $abort('no arm matched');
    }
    $t9=c_16;
  }else{
    $t9=0n;
  }
  const last_4=$t9;
  const text_19=String(head_2)+' '+String(pair_3)+' '+String(last_4);
  const self_20=$host_HostStdout_println(ctx_1[1],text_19);
  let $t13;
  if(self_20[0]===0){
    $t13=0;
  }else if(self_20[0]===1){
    $t13=0;
  }else{
    $abort('no arm matched');
  }
  return $k1;
}
function __cmd_x_main_buri$upTo$l3wgis(ctx_0,n_1,acc_2){
  while(true){
    if(n_1===0n){
      return $list_reverse(acc_2,ctx_0);
    }else{
      const $t1=n_1-1n;
      acc_2=$list_push(acc_2,ctx_0,n_1);
      n_1=$t1;
      continue;
    }
  }
}
